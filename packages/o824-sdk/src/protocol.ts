// The wire types, as the kernel and the recordings write them.
//
// These are hand-written for 0.1 and pinned by a contract test that parses every
// message in `spec/transcripts/*.jsonl`. The design calls for generating them from a
// JSON Schema; that schema does not exist yet, so the test is what keeps the types
// honest until it does.

/** One installation the kernel found. */
export interface Install {
  installId: string;
  kind: 'desktop' | 'web' | 'tui' | string;
  root: string;
  version: string;
  profiles: string[];
  health: 'ok' | 'degraded' | 'damaged' | string;
  home: string;
}

/** How bad a finding is. */
export type Severity = 'info' | 'warning' | 'error';

/** One diagnosis. `params` carries the values a UI renders into a localized message. */
export interface Finding {
  checkId: string;
  severity: Severity;
  code: string;
  category: string;
  params?: Record<string, unknown>;
  fallbackMessage?: string;
  detail?: string;
  repair?: { kind: string; planHint?: string };
}

/** The doctor output. */
export interface Envelope {
  envelope: 'dsh-doctor/v1';
  findings: Finding[];
}

/** One step of a plan. */
export interface Step {
  kind: string;
  path: string;
  from?: string;
  to?: string;
  args?: string[];
  detail?: string;
}

/** A repair plan: what would change, and the fingerprint it was built against. */
export interface Plan {
  id: string;
  installId: string;
  root: string;
  createdAt: string;
  expiresAt: string;
  steps: Step[];
  fingerprint: Record<string, string>;
}

/** The `o824.info` result: what this kernel is and what it can do. */
export interface KernelInfo {
  protocolVersion: string;
  minClientProtocol: string;
  impl: { name: string; version: string };
  capabilities: string[];
  platform: { os: string; arch: string };
}

/** A job lifecycle notification (`op.state`). */
export interface JobState {
  seq: number;
  jobId: string;
  state: 'running' | 'done' | 'failed' | 'cancelled';
  phase?: string;
  error?: { code: string; message: string };
}

/** A job progress notification (`op.progress`). */
export interface JobProgress {
  seq: number;
  jobId: string;
  phase: string;
  done: number;
  total: number;
  indeterminate: boolean;
}

/** A job result notification (`op.result`). */
export interface JobResult {
  seq: number;
  jobId: string;
  [key: string]: unknown;
}

/** A JSON-RPC error, exactly as the kernel sends it. */
export interface RpcErrorBody {
  code: number;
  message: string;
  /** Optional, and explicitly allowed to be absent: `exactOptionalPropertyTypes` makes
   * "no data" and "data: undefined" different types, and the kernel sends the former. */
  data?: { code?: string; message?: string; [key: string]: unknown } | undefined;
}

/** Any message on the wire. */
export type WireMessage =
  | { jsonrpc: '2.0'; id: number; method: string; params?: unknown }
  | { jsonrpc: '2.0'; id: number; seq?: number; result?: unknown; error?: RpcErrorBody }
  | { jsonrpc: '2.0'; method: string; params?: Record<string, unknown> };

/**
 * Throws when `value` is not a message this SDK understands.
 *
 * Used by the contract test, not by the client: the client must tolerate unknown
 * fields (forward compatibility) but a recording that does not parse means the types
 * and the wire have drifted apart.
 */
export function assertMessage(value: unknown): asserts value is WireMessage {
  if (typeof value !== 'object' || value === null) {
    throw new Error('a frame body must be a JSON object');
  }
  const message = value as Record<string, unknown>;
  if (message.jsonrpc !== '2.0') {
    throw new Error(`a message must say "jsonrpc":"2.0", got ${JSON.stringify(message.jsonrpc)}`);
  }
  const hasMethod = typeof message.method === 'string';
  const hasId = typeof message.id === 'number';
  const hasResult = 'result' in message;
  const hasError = 'error' in message;
  if (!hasMethod && !hasResult && !hasError) {
    throw new Error('a message must be a request, a notification, or a response');
  }
  if (hasResult && hasError) {
    throw new Error('a response carries exactly one of result or error');
  }
}
