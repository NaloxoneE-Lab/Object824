// The launcher's decisions, kept pure so they can be tested without a guest.
//
// WebAssembly has no ambient filesystem: WASI only sees directories the host
// preopens, and it sees them at guest paths. So a transcript argument has to become
// two things — a preopened directory and a guest path inside it — and that mapping is
// the whole contract between this package and the user's paths.
//
// The rules are deliberately narrow:
//   * something that looks like a path (ends with .jsonl, or contains a separator)
//     is resolved against the caller's working directory and its directory is
//     preopened;
//   * anything else must be one of the packaged recordings, named without a path.
// An unknown name is refused here, with the list, rather than handed to the guest to
// fail on a file it cannot even describe.

import { basename, dirname, join, resolve } from 'node:path';

/** The recordings the package ships, selectable by name. */
export const BUNDLED_TRANSCRIPTS = ['doctor-clean', 'doctor-findings', 'plan-apply'];

/** A refusal that carries the exit code the process should use. */
export class LauncherError extends Error {
  constructor(message, exitCode = 2) {
    super(message);
    this.name = 'LauncherError';
    this.exitCode = exitCode;
  }
}

/** The guest path the transcript directory is preopened at. */
export const GUEST_DIR = '/t';

/** The minimum Node this package supports. */
export const MINIMUM_NODE_MAJOR = 20;

/** The characters that make a value a path rather than a recording name. */
const SEPARATORS = ['/', String.fromCharCode(92)];

/**
 * Returns a sentence when `version` is too old for this package, or null when it is
 * supported. The sentence names the floor, because "unexpected token" from inside the
 * WASI API is not a useful thing to read.
 */
export function requireNode20(version) {
  const major = Number.parseInt(String(version).split('.')[0], 10);
  if (!Number.isFinite(major) || major < MINIMUM_NODE_MAJOR) {
    return (
      `o824-mock needs Node ${MINIMUM_NODE_MAJOR} or newer (it runs the mock as WebAssembly ` +
      `through node:wasi); this is Node ${version}.`
    );
  }
  return null;
}

/**
 * Turns the caller's argv into what the guest should receive: the same arguments with
 * `--transcript` rewritten to a guest path, plus the host directory to preopen.
 */
export function describeTranscript(argv, packageDir) {
  const args = [...argv];
  const position = args.indexOf('--transcript');
  if (position === -1) {
    throw new LauncherError('pass --transcript <path|name>');
  }
  if (position + 1 >= args.length) {
    throw new LauncherError('--transcript needs a value');
  }

  const value = args[position + 1];
  const looksLikeAPath =
    value.endsWith('.jsonl') || SEPARATORS.some((separator) => value.includes(separator));

  if (looksLikeAPath) {
    const absolute = resolve(value);
    args[position + 1] = `${GUEST_DIR}/${basename(absolute)}`;
    return { argv: args, preopen: dirname(absolute), hostFile: absolute };
  }

  if (!BUNDLED_TRANSCRIPTS.includes(value)) {
    throw new LauncherError(
      `unknown recording ${JSON.stringify(value)}; the package ships ${BUNDLED_TRANSCRIPTS.join(', ')}`,
    );
  }
  args[position + 1] = `${GUEST_DIR}/${value}.jsonl`;
  return {
    argv: args,
    preopen: join(packageDir, 'transcripts'),
    hostFile: join(packageDir, 'transcripts', `${value}.jsonl`),
  };
}
