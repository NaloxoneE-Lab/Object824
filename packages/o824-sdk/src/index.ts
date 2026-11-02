// The O824 kernel client: one child process, JSON-RPC over its stdio.
//
// The layering is the design's (§7.5): the kernel is always a separate process, the
// protocol is the only surface, and this SDK is a thin client for it. It deliberately
// provides no "should I repair this?" judgement and no confirmation — those belong to
// the UI, and a client that guessed would be a second, disagreeing brain.

export { encodeFrame, FrameDecoder, FrameError, MAX_FRAME_BYTES } from './frames.js';
export type {
  Envelope,
  Finding,
  Install,
  JobProgress,
  JobResult,
  JobState,
  KernelInfo,
  Plan,
  RpcErrorBody,
  Severity,
  Step,
  WireMessage,
} from './protocol.js';
export { assertMessage } from './protocol.js';
export { connect, ConnectionClosedError, KernelClient, RpcError } from './client.js';
export type { ConnectOptions, JobEvent } from './client.js';
