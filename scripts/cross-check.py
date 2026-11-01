"""Cross-check the two implementations on every fixture kind.

For each kind this script:

1. materialises the tree with the Rust fixture builder,
2. runs the Go oracle's `doctor --json` on it,
3. runs the Rust binary's `doctor --json` on it,
4. compares Go against the golden file, Rust against the golden file, and the
   two implementations against each other.

That third comparison is the interesting one: it proves the Rust fixture builder
produces a tree the oracle diagnoses exactly as the committed contract says. The
in-repo golden test can only compare Rust against the file; only this script can
say the oracle agrees, and only this script would notice a fixture builder that
drifts away from the one the goldens were generated with.

Usage (from `o824-rs`):

    cargo build --release --bin o824
    python scripts/cross-check.py

It needs the frozen Go reference built as `../o824.exe`; if it is missing the
script says so instead of guessing.
"""

import json
import os
import pathlib
import shutil
import subprocess
import sys

RS = pathlib.Path(__file__).resolve().parent.parent
REPO = RS.parent
GO_BIN = REPO / "o824.exe"
RUST_BIN = RS / "target" / "release" / "o824.exe"
BASE = RS / ".toolchain" / "cross-run"
GOLDEN = RS / "spec" / "golden"
KINDS = ["ok", "broken-link", "missing-bundle", "dup-loader", "parse-error",
         "half-install", "dangling-bridge", "git-dep"]


def materialise(kind, directory):
    subprocess.run(
        ["cargo", "run", "-q", "--manifest-path", str(RS / "Cargo.toml"),
         "-p", "o824-fixtures", "--example", "materialize", "--", kind, str(directory)],
        check=True, capture_output=True,
    )


def doctor(binary, root, data_root):
    environment = dict(os.environ)
    environment["O824_DATA_ROOT"] = str(data_root)
    result = subprocess.run(
        [str(binary), "doctor", "--root", str(root), "--home", str(root), "--json"],
        capture_output=True, text=True, env=environment,
    )
    return result.returncode, result.stdout, result.stderr


def normalize(document, root):
    text = json.dumps(document, ensure_ascii=False)
    text = text.replace(str(root).replace("\\", "\\\\"), "<ROOT>")
    text = text.replace(str(root), "<ROOT>")
    return text


def main():
    for required in (GO_BIN, RUST_BIN):
        if not required.is_file():
            print(f"missing {required}; build it first "
                  f"(cargo build --release --bin o824 / go build -o o824.exe ./cmd/o824)")
            return 2

    if BASE.exists():
        shutil.rmtree(BASE, ignore_errors=True)
    BASE.mkdir(parents=True)

    failures = 0
    for kind in KINDS:
        root = BASE / kind
        root.mkdir()
        materialise(kind, root)

        data_root = BASE / f"data-{kind}"
        data_root.mkdir()
        journal = root / "journal"
        if journal.is_dir():
            shutil.copytree(journal, data_root / "journal")

        go_code, go_out, go_err = doctor(GO_BIN, root, data_root)
        rs_code, rs_out, rs_err = doctor(RUST_BIN, root, data_root)

        golden = json.loads((GOLDEN / f"{kind}.doctor.json").read_text(encoding="utf-8"))
        go_doc = json.loads(go_out)
        rs_doc = json.loads(rs_out)

        verdicts = {
            "go==golden": normalize(go_doc, root) == normalize(golden, root),
            "rust==golden": normalize(rs_doc, root) == normalize(golden, root),
            "go==rust": normalize(go_doc, root) == normalize(rs_doc, root),
            "exit": go_code == rs_code,
        }
        ok = all(verdicts.values())
        failures += 0 if ok else 1
        print(f"{kind:16}", "PASS" if ok else "FAIL " + json.dumps(verdicts))
        if not ok:
            print("  go   :", normalize(go_doc, root)[:600])
            print("  rust :", normalize(rs_doc, root)[:600])
            print("  goerr:", go_err[:300])
            print("  rserr:", rs_err[:300])

    print(f"\n{len(KINDS) - failures}/{len(KINDS)} kinds agree")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
