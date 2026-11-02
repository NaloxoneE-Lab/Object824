# o824-mock

Replays a recorded [O824](../../docs/design/2026-10-28-dsh-launcher-design.md) session
so you can build and test a UI against the real protocol — **without DSH installed,
without a broken machine, and without Windows**.

The replayer is the kernel's own: the same Rust code is compiled to WebAssembly and run
under Node, so the frames you see are the frames the kernel would send.

## Install

From a release (no npm account needed):

```bash
npm i -D https://github.com/NaloxoneE-Lab/Object824/releases/download/v0.1.2/o824-mock-0.1.2.tgz
```

Then:

```bash
npx o824-mock --transcript doctor-clean
```

Requires **Node 20 or newer**. Nothing else — no DSH, no build tools, no Windows.

## Use

The mock is a child process that speaks
[LSP-style framing](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#headerPart):
`Content-Length: <n>\r\n\r\n<json>`, one JSON-RPC 2.0 message per frame. It reads client
frames on stdin and writes server frames on stdout.

```js
import { spawn } from 'node:child_process';

const child = spawn('npx', ['-y', 'o824-mock', '--transcript', 'plan-apply'],
                    { stdio: ['pipe', 'pipe', 'inherit'], shell: true });

let buffer = Buffer.alloc(0);
const replies = new Map();

child.stdout.on('data', (chunk) => {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const end = buffer.indexOf('\r\n\r\n');
    if (end < 0) return;
    const length = Number(/Content-Length:\s*(\d+)/i.exec(buffer.subarray(0, end))[1]);
    if (buffer.length < end + 4 + length) return;

    const message = JSON.parse(buffer.subarray(end + 4, end + 4 + length));
    buffer = buffer.subarray(end + 4 + length);

    if (message.id !== undefined) replies.get(message.id)?.(message);
    else console.log('event', message.method, message.params.seq); // op.state / op.progress / op.result
  }
});

let id = 1;
const send = (method, params) => new Promise((resolve) => {
  const body = Buffer.from(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  child.stdin.write(`Content-Length: ${body.length}\r\n\r\n${body}`);
  replies.set(id++, resolve);
});

await send('o824.info', { clientProtocol: '0.1.0' });
const discover = await send('o824.discover');
console.log(discover.result.installs);
```

**The same client code drives the real kernel**: replace the command with `o824` and
nothing else changes. That is the point of the mock — develop the whole UI here, then
point it at a machine with DSH.

## Options

| Flag | Meaning |
|---|---|
| `--transcript <path\|name>` | A recording to replay. A bare name picks one of the bundled recordings: `doctor-clean`, `doctor-findings`, `plan-apply`. |
| `--delay <ms>` | Pause between emitted server messages, so a progress bar has something to show. |
| `--fail-at <n>` | Replace the n-th emitted message with `-32603` / `E_MOCK_FAILURE`, so you can build and test your error path. |
| `--protocol <ver>` | Report this `protocolVersion` from `o824.info` instead of the recorded one, to test negotiation. |

Exit codes: `0` the client ended the session, `2` the command line was unusable, `8`
the recording could not be read.

Requests are matched **by method**, not by position. Each recorded answer is used once,
in recorded order; asking for a method more times than the recording holds keeps
replaying its last answer, so re-sending `info` or `discover` while developing does not
dead-end you. A method the recording does not cover is answered with `-32601` /
`E_UNSUPPORTED` and `data.supported` naming the ones it does.

## Record your own

A transcript is JSONL: one JSON-RPC message per line, in session order. A line with
`method` and `id` is a client request; `method` alone is a server notification; `id`
with exactly one of `result` or `error` is a server response. The three shipped files
in `spec/transcripts/` are the format by example.

## Where it comes from

- The replayer: [`crates/o824-mock`](../../crates/o824-mock) — the same crate `o824 mock`
  uses.
- The path rules: [`lib/launcher.mjs`](lib/launcher.mjs).
- Why WebAssembly: it keeps one implementation instead of a second, drifting one, and it
  is what makes "works on macOS and Linux" true.
- The protocol: `docs/design/2026-10-28-dsh-launcher-design.md` §9 and §19.
