# o824-sdk

A thin TypeScript client for the O824 kernel's stdio protocol. It does the seven things
a client has to do — framing, request/response correlation, job events, cancellation,
process-exit detection, capability negotiation, and the wire types — and nothing else.

**It never decides anything for you.** No "should I repair this", no confirmation on
your behalf, no automatic retry of a mutating request. Those belong to the UI, and a
client that guessed would be a second brain disagreeing with yours.

## Install

```bash
npm i -D https://github.com/NaloxoneE-Lab/Object824/releases/download/v0.1.2/o824-sdk-0.1.0.tgz
```

Requires **Node 20 or newer**. No runtime dependencies.

## Use

```ts
import { connect } from 'o824-sdk';

const kernel = connect({
  command: 'o824.exe',
  args: ['kernel', '--stdio'],
});

kernel.on((event) => {
  if (event.kind === 'progress') {
    console.log(event.progress.phase, event.progress.done, '/', event.progress.total);
  }
});

const info = await kernel.info();            // protocolVersion + capabilities
const installs = await kernel.discover();    // every installation on the machine
const envelope = await kernel.doctor({ installId: installs[0].installId });

if (envelope.findings.length > 0) {
  const plan = await kernel.plan({ installId: installs[0].installId });

  // Confirming is the UI's job, and this line is the UI doing it.
  if (await askTheUser(plan)) {
    await kernel.apply({
      planId: plan.id,
      confirm: true,
      idempotencyKey: crypto.randomUUID(),
    });
  }
}

await kernel.close();
```

## The kernel server is the next milestone

`o824 kernel --stdio` does not exist yet — the protocol has a codec and a recorded
replayer, and the live server is the piece still to come. Until it lands, point the
client at the mock, which speaks the same protocol from a recording:

```ts
import { connect } from 'o824-sdk';

const kernel = connect({
  command: process.execPath,
  args: ['node_modules/o824-mock/bin/o824-mock.mjs', '--transcript', 'doctor-clean'],
});
```

That is how this package's own tests run, so a client written against the mock will not
have to change when the server arrives.

## What it refuses to do

| Not doing this | Because |
|---|---|
| Deciding whether a finding should be repaired | That is the UI's judgement, with the user's context |
| Confirming on the caller's behalf | `apply` requires `confirm: true` from whoever asked |
| Retrying `apply` | A retried apply is a second repair; call again if you mean it |
| Rewriting kernel errors | You get `code`, `data.code`, and the message as sent — no "friendly" paraphrase |

## Errors

```ts
import { RpcError, ConnectionClosedError } from 'o824-sdk';

try {
  await kernel.request('o824.start', { installId: 'i1' });
} catch (error) {
  if (error instanceof RpcError) {
    console.log(error.code, error.kernelCode, error.data);  // -32601, E_UNSUPPORTED, { supported: [...] }
  } else if (error instanceof ConnectionClosedError) {
    // The kernel is gone; the message says to connect again.
  }
}
```

## Types

`Install`, `Finding`, `Envelope`, `Plan`, `Step`, `KernelInfo`, `JobState`,
`JobProgress`, `JobResult`, and `WireMessage`. They are hand-written for 0.1 and pinned
by a contract test that parses every message in `spec/transcripts/`; the design calls
for generating them from a JSON Schema, which is a later step that will not change how
you use them.
