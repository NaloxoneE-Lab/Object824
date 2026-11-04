#!/usr/bin/env bash
# Makes a releasable folder: both executables, a signed manifest, and a verification that
# fails the script if anything is off.
#
#   scripts/release.sh <version> <secret-key-file>
#
# Produces dist/<version>/ containing:
#   o824.exe, o824-gui.exe   the two programs
#   manifest.json            every file's sha256, signed with the key you gave
#
# and prints the public key, the manifest's own sha256, and the three things a release page
# needs to say. Nothing is uploaded: publishing is a decision, not a side effect of building.
#
# The GUI is built with O824_RELEASE_KEY set to the matching public key, so `o824 update
# --check` in the released build trusts the manifests this key signs. That is the whole point
# of building it here rather than copying a development binary.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-}"
KEY_FILE="${2:-}"

if [ -z "$VERSION" ] || [ -z "$KEY_FILE" ]; then
  echo "usage: scripts/release.sh <version> <secret-key-file>" >&2
  echo "  make a key once with: o824 manifest-keygen --out release.key" >&2
  exit 1
fi
if [ ! -f "$KEY_FILE" ]; then
  echo "release: no such key file: $KEY_FILE" >&2
  exit 1
fi
case "$VERSION" in
  v*|[0-9]*) ;;
  *) echo "release: version should look like 0.2.0 or v0.2.0, got '$VERSION'" >&2; exit 1 ;;
esac

cd "$ROOT"
OUT="dist/$VERSION"
O824="$ROOT/target/release/o824"

echo "== public key for this release =="
# A bootstrap build, only to read the public half out of the key file: the tool that derives it
# is the very binary we are about to replace.
source scripts/env.sh >/dev/null
cargo build --release --bin o824
O824="$ROOT/target/release/o824"
PUBLIC="$("$O824" manifest-keygen --from "$KEY_FILE" | sed -n 's/^public key: //p')"
export O824_RELEASE_KEY="$PUBLIC"
if [ -z "$PUBLIC" ]; then
  echo "release: could not read a public key from $KEY_FILE" >&2
  exit 1
fi
echo "   $PUBLIC"

echo
echo "== rebuild the kernel, trusting that key =="
# `option_env!` is read at compile time and a warm cache can satisfy a build without
# recompiling, which would leave the keyless binary behind; cleaning the crate that holds the
# constant makes the question unavoidable.
cargo clean -p o824-manifest >/dev/null 2>&1 || true
cargo build --release --bin o824
O824="$ROOT/target/release/o824"

echo
echo "== build the window, trusting that key =="
# The GUI needs the MSVC toolchain; this is the same environment the GUI is developed in.
(
  source scripts/env.sh >/dev/null
  source scripts/env-msvc.sh >/dev/null
  export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc
  export O824_RELEASE_KEY="$PUBLIC"
  cd apps/o824-gui
  cargo build --release
)
GUI="$ROOT/apps/o824-gui/target/release/o824-gui.exe"
if [ ! -f "$GUI" ]; then
  echo "release: the window was not built: $GUI" >&2
  exit 1
fi

echo
echo "== stage $OUT =="
rm -rf "$OUT"
mkdir -p "$OUT"
cp "$O824" "$OUT/o824.exe"
cp "$GUI" "$OUT/o824-gui.exe"
ls -la "$OUT" | sed 's/^/   /'

echo
echo "== sign the manifest =="
"$O824" manifest --root "$OUT" --out "$OUT/manifest.json" --version "$VERSION" --key-file "$KEY_FILE"

echo
echo "== verify what was just written, against the published key =="
"$O824" verify-manifest --manifest "$OUT/manifest.json" --root "$OUT" --public-key "$PUBLIC"

echo
echo "== does the released kernel trust its own key? =="
# Without this, the release signs a manifest that its own binary will only accept as
# "self-signed" - the mistake this check exists to catch.
# `update --check` exits 1 when something newer is published, which is the normal case here;
# with `pipefail` that would decide the pipeline's status and make this check lie. The text is
# what matters, so it is captured first and inspected second.
VERDICT="$("$OUT/o824.exe" update --check --manifest "$OUT/manifest.json" --version 0.0.0 2>&1 || true)"
if ! printf '%s' "$VERDICT" | grep -q "built into this binary"; then
  echo "release: the released binary does not carry the release key" >&2
  printf '%s
' "$VERDICT" >&2
  exit 1
fi
echo "   ok: the binary carries the key"

echo
echo "== what the release page needs =="
MANIFEST_SHA="$(sha256sum "$OUT/manifest.json" | cut -d' ' -f1)"
cat <<TEXT
   version        : $VERSION
   public key     : $PUBLIC
   manifest sha256: $MANIFEST_SHA

   How to install:

       1. extract the folder anywhere;
       2. run:  o824.exe install
          (copies both programs into %LOCALAPPDATA%\o824, adds a Start Menu shortcut, and
           registers a removal entry - no administrator rights needed);
       3. launch O824 from the Start Menu.

   Tell people, on the page and in the first screen:

     1. this build is unsigned, so Windows SmartScreen will show
        "unrecognised app" the first time; the warning is about identity, not damage;
     2. how to check the download:
          o824 verify-manifest --manifest manifest.json --root <folder> --public-key $PUBLIC
     3. never turn a protection off for us - if something asks you to, it is not us.

   Publish $OUT/manifest.json next to the executables. Nothing has been uploaded: that is
   your decision, not this script's.
TEXT
