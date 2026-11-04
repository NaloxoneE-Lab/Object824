#!/usr/bin/env bash
# Makes a release checkable: build the manifest for a directory, sign it, then verify what
# was written — the same three steps a stranger would follow with the published key.
#
#   scripts/release-manifest.sh <release-dir> <secret-key-file> [public-key-hex]
#
# The secret key is read from a file and never printed. When the public key is given, the
# verification uses **that** key rather than the one inside the manifest, which is the only
# version of the check that means anything to a downloader.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
O824="$ROOT/target/release/o824"
if [ ! -x "$O824" ] && [ -x "$O824.exe" ]; then O824="$O824.exe"; fi
if [ ! -x "$O824" ]; then
  echo "release-manifest: build first: cargo build --release --bin o824" >&2
  exit 1
fi

RELEASE="${1:-}"
KEY="${2:-}"
PUBLIC="${3:-}"
if [ -z "$RELEASE" ] || [ -z "$KEY" ]; then
  echo "usage: scripts/release-manifest.sh <release-dir> <secret-key-file> [public-key-hex]" >&2
  exit 1
fi

echo "== manifest =="
"$O824" manifest --root "$RELEASE" --out "$RELEASE/manifest.json" --key-file "$KEY"
echo
echo "== verify =="
if [ -n "$PUBLIC" ]; then
  "$O824" verify-manifest --manifest "$RELEASE/manifest.json" --root "$RELEASE" --public-key "$PUBLIC"
else
  echo "note: no public key given, so this only proves the manifest is self-consistent"
  "$O824" verify-manifest --manifest "$RELEASE/manifest.json" --root "$RELEASE"
fi
