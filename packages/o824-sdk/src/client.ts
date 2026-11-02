// The client: one child process, frames on its stdio, JSON-RPC in both directions.
//
// What it does (§7.5's "must do"): frame, correlate requests with responses, turn job
// notifications into events, propagate cancellation, notice when the process died and
// say how to reconnect, and answer "can this kernel do X" from `o824.info`.
//
// What it refuses to do: decide whether something should be repaired, confirm on the
// user's behalf, or retry a mutating call. A retried `apply` is a second repair.

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';

import { encodeFrame, FrameDecoder } from './frames.js';
import type {
  Envelope,
  Install,
  JobProgress,
  JobResult,
  JobState,
  KernelInfo,
  Plan,
  RpcErrorBody,
} from './protocol.js';

/** How to start the server. */
export interface ConnectOptions {
  /** The executable to run. */
  command: string;
  /** Its arguments, e.g. `['kernel', '--stdio']`. */
  args?: string[];
  /** Working directory for the child. */
  cwd?: string;
  /** Extra environment for the child. */
  env?: NodeJS.ProcessEnv;
  /** How long a request may take before it is rejected. Zero waits forever. */
  requestTimeoutMs?: number;
}

/** One job notification. */
export interface JobEvent {
  jobId: string;
  /** `state`, `progress`, or `result`. */
  kind: 'state' | 'progress' | 'result';
  state?: JobState;
  progress?: JobProgress;
  result?: JobResult;
}

/** A JSON-RPC error, rethrown as the kernel sent it. */
export class RpcError extends Error {
  readonly code: number;
  readonly data: RpcErrorBody['data'];

  constructor(body: RpcErrorBody) {
    super(body.message);
    this.name = 'RpcError';
    this.code = body.code;
    this.data = body.data;
  }

  /** The kernel's own error code, when it sent one (`E_UNSUPPORTED`, …). */
  get kernelCode(): string | undefined {
    return this.data?.code;
  }
}

/** The kernel process is gone; a new client is the only way forward. */
export class ConnectionClosedError extends Error {
  readonly exitCode: number | null;

  constructor(exitCode: number | null) {
    super(
      exitCode === null
        ? 'the O824 kernel closed its pipes; connect again to continue'
        : `the O824 kernel exited with code ${exitCode}; connect again to continue`,
    );
    this.name = 'ConnectionClosedError';
    this.exitCode = exitCode;
  }
}

type Pending = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  method: string;
  timer?: NodeJS.Timeout;
};

/** A connected kernel. */
export class KernelClient {
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly decoder = new FrameDecoder();
  private readonly pending = new Map<number, Pending>();
  private readonly listeners = new Set<(event: JobEvent) => void>();
  /**
   * Events that arrived before anyone asked for them.
   *
   * A kernel may answer `o824.doctor` and emit the whole job — progress, result, done —
   * in one write. The promise for the answer resolves in a microtask, so by the time a
   * caller says "now wait for the job" the result is already gone. Buffering is not an
   * optimisation here; without it every fast job hangs.
   */
  private readonly jobEvents = new Map<string, JobEvent[]>();
  private nextId = 1;
  private closed = false;
  private readonly requestTimeoutMs: number;
  /**
   * Resolves once the process is gone *and its pipes are closed*.
   *
   * `exit` fires before the stdio streams finish closing, so waiting for it leaves
   * handles behind — which in a long-lived process is a leak and in a test runner is a
   * hang. `close` is the event that means "nothing of this child is left".
   *
   * It is created with the child, not when someone asks: attaching a listener after
   * `kill()` can miss an exit that already happened and then wait for ever.
   */
  private readonly exited: Promise<number | null>;

  constructor(child: ChildProcessWithoutNullStreams, requestTimeoutMs = 0) {
    this.child = child;
    this.requestTimeoutMs = requestTimeoutMs;
    this.exited = new Promise<number | null>((resolve) => child.once('close', (code) => resolve(code)));

    child.stdout.on('data', (chunk: Buffer) => this.onData(chunk));
    // The kernel's stderr is diagnostics, never protocol (§7.5).
    child.stderr.on('data', () => {});
    child.on('exit', (code) => this.onExit(code));
    child.on('error', () => this.onExit(null));
  }

  /** Every job notification, in arrival order. */
  on(listener: (event: JobEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** `o824.info`: protocol version and capabilities. */
  async info(): Promise<KernelInfo> {
    return (await this.request('o824.info', { clientProtocol: '0.1.0' })) as KernelInfo;
  }

  /**
   * The capabilities this kernel reports, as a plain lookup.
   *
   * Asking the kernel rather than a version table is the point: a client must be able
   * to work with a kernel older or newer than itself.
   */
  async capabilities(): Promise<Record<string, boolean>> {
    const info = await this.info();
    const table: Record<string, boolean> = {};
    for (const name of info.capabilities ?? []) {
      table[name] = true;
    }
    return table;
  }

  /** `o824.discover`: every installation on the machine. */
  async discover(): Promise<Install[]> {
    const result = (await this.request('o824.discover')) as { installs: Install[] };
    return result.installs;
  }

  /** `o824.doctor`: start a diagnosis job and wait for its result. */
  async doctor(params: { installId: string; profile?: string }): Promise<Envelope> {
    return (await this.runJob('o824.doctor', params)) as Envelope;
  }

  /** `o824.plan`: build a plan and wait for it. Building one changes nothing. */
  async plan(params: {
    installId: string;
    profile?: string;
    withInstall?: boolean;
  }): Promise<Plan> {
    return (await this.runJob('o824.plan', params)) as Plan;
  }

  /**
   * `o824.apply`: the one call that changes the machine.
   *
   * It is never retried here: a second apply of the same plan is either rejected by the
   * kernel or a second repair, and neither is what a caller wanted when a request timed
   * out. A caller that wants to try again calls again.
   */
  async apply(params: {
    planId: string;
    confirm: boolean;
    idempotencyKey: string;
  }): Promise<JobResult> {
    return (await this.runJob('o824.apply', params)) as JobResult;
  }

  /**
   * `o824.cancel`: asks the kernel to stop a job.
   *
   * This resolves when the request was accepted, not when the job stopped — the job's
   * own `state` notification is the only proof of that. A kernel without cancellation
   * answers `-32601`, which surfaces here as an `RpcError`.
   */
  async cancel(jobId: string): Promise<void> {
    await this.request('o824.cancel', { jobId });
  }

  /** Sends one request and resolves with its result. */
  async request(method: string, params?: unknown): Promise<unknown> {
    if (this.closed) {
      throw new ConnectionClosedError(this.exitCode);
    }
    const id = this.nextId++;
    const message: Record<string, unknown> = { jsonrpc: '2.0', id, method };
    if (params !== undefined) {
      message.params = params;
    }

    return new Promise<unknown>((resolve, reject) => {
      const entry: Pending = { resolve, reject, method };
      if (this.requestTimeoutMs > 0) {
        entry.timer = setTimeout(() => {
          this.pending.delete(id);
          reject(new Error(`${method}: no answer within ${this.requestTimeoutMs} ms`));
        }, this.requestTimeoutMs);
      }
      this.pending.set(id, entry);
      this.child.stdin.write(encodeFrame(message));
    });
  }

  /**
   * Terminates the kernel process.
   *
   * The design puts the kernel in a Job Object so it dies with its UI; a client that
   * wants to stop it early uses this. Pending requests are rejected, not left hanging.
   */
  async kill(): Promise<void> {
    if (this.closed) {
      return;
    }
    this.child.kill();
    await this.exited;
  }

  /** Closes stdin and waits for the kernel to exit. */
  async close(): Promise<void> {
    if (this.closed) {
      return;
    }
    this.child.stdin.end();
    await this.exited;
  }

  /** Runs a job-style method: the answer is a `jobId`, the result arrives as events. */
  private async runJob(method: string, params: unknown): Promise<unknown> {
    const { jobId } = (await this.request(method, params)) as { jobId: string };
    return this.waitForJob(jobId);
  }

  /** Resolves with a job's `op.result`, rejects when it fails or the process dies. */
  private waitForJob(jobId: string): Promise<unknown> {
    return new Promise<unknown>((resolve, reject) => {
      let settled = false;
      const settle = (finish: () => void) => {
        if (settled) {
          return;
        }
        settled = true;
        this.jobEvents.delete(jobId);
        finish();
      };
      const handle = (event: JobEvent) => {
        if (event.kind === 'result') {
          settle(() => resolve(event.result));
        } else if (event.kind === 'state' && event.state?.state === 'failed') {
          const error = event.state.error;
          settle(() =>
            reject(new RpcError({ code: -32000, message: error?.message ?? 'the job failed', data: error })),
          );
        } else if (event.kind === 'state' && event.state?.state === 'cancelled') {
          settle(() => reject(new Error(`${jobId}: cancelled`)));
        }
      };

      // Replay what arrived before this call: a fast job may already be over.
      for (const event of this.jobEvents.get(jobId) ?? []) {
        handle(event);
      }
      if (settled) {
        return;
      }

      const stop = this.on((event) => {
        if (event.jobId !== jobId) {
          return;
        }
        handle(event);
        if (settled) {
          stop();
        }
      });
    });
  }

  private onData(chunk: Buffer): void {
    for (const message of this.decoder.push(chunk)) {
      this.dispatch(message as Record<string, unknown>);
    }
  }

  private dispatch(message: Record<string, unknown>): void {
    if (typeof message.method === 'string') {
      this.notify(message);
      return;
    }
    const id = message.id;
    if (typeof id !== 'number') {
      return;
    }
    const entry = this.pending.get(id);
    if (!entry) {
      return;
    }
    this.pending.delete(id);
    if (entry.timer) {
      clearTimeout(entry.timer);
    }
    if (message.error) {
      entry.reject(new RpcError(message.error as RpcErrorBody));
    } else {
      entry.resolve(message.result);
    }
  }

  private notify(message: Record<string, unknown>): void {
    const params = (message.params ?? {}) as Record<string, unknown>;
    const jobId = typeof params.jobId === 'string' ? params.jobId : undefined;
    if (jobId === undefined) {
      return;
    }
    let kind: JobEvent['kind'];
    switch (message.method) {
      case 'op.state':
        kind = 'state';
        break;
      case 'op.progress':
        kind = 'progress';
        break;
      case 'op.result':
        kind = 'result';
        break;
      default:
        return;
    }
    const event: JobEvent = { jobId, kind };
    if (kind === 'state') {
      event.state = params as unknown as JobState;
    } else if (kind === 'progress') {
      event.progress = params as unknown as JobProgress;
    } else {
      event.result = params as unknown as JobResult;
    }
    // Keep the event for a caller that has not asked for it yet; bounded, because a
    // long session with many jobs must not grow without limit.
    const buffered = this.jobEvents.get(jobId) ?? [];
    buffered.push(event);
    this.jobEvents.delete(jobId);
    this.jobEvents.set(jobId, buffered);
    while (this.jobEvents.size > 64) {
      const oldest = this.jobEvents.keys().next();
      if (oldest.done) {
        break;
      }
      this.jobEvents.delete(oldest.value);
    }

    for (const listener of this.listeners) {
      listener(event);
    }
  }

  private exitCode: number | null = null;

  private onExit(code: number | null): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.exitCode = code;
    for (const entry of this.pending.values()) {
      if (entry.timer) {
        clearTimeout(entry.timer);
      }
      entry.reject(new ConnectionClosedError(code));
    }
    this.pending.clear();
  }
}

/** Starts the kernel and connects to it. */
export function connect(options: ConnectOptions): KernelClient {
  const child = spawn(options.command, options.args ?? [], {
    cwd: options.cwd,
    env: options.env ?? process.env,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  }) as ChildProcessWithoutNullStreams;
  return new KernelClient(child, options.requestTimeoutMs ?? 0);
}
