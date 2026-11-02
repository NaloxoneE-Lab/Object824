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


def normalize_plan(document, root):
    """Replace the values no golden can contain, exactly like the Rust harness."""
    text = json.dumps(document, ensure_ascii=False)
    replacements = [
        (str(root).replace("\\", "\\\\"), "<ROOT>"),
        (str(root), "<ROOT>"),
        (document.get("id", ""), "<PLAN_ID>"),
        (document.get("installId", ""), "<INSTALL_ID>"),
        (document.get("createdAt", ""), "<TIMESTAMP>"),
        (document.get("expiresAt", ""), "<TIMESTAMP>"),
    ]
    # Longest first, so a shorter needle cannot eat part of a longer one.
    for needle, placeholder in sorted(replacements, key=lambda pair: -len(pair[0])):
        if needle:
            text = text.replace(needle, placeholder)
    return text


def plan_checks():
    """Compare `repair --dry-run --json` against the plan goldens, all three ways."""
    print("\n== plan goldens ==")
    failures = 0
    for kind in KINDS:
        root = BASE / f"plan-{kind}"
        root.mkdir()
        materialise(kind, root)

        data_root = BASE / f"plan-data-{kind}"
        data_root.mkdir()
        journal = root / "journal"
        if journal.is_dir():
            shutil.copytree(journal, data_root / "journal")

        go = run_json(GO_BIN, ["repair", "--root", str(root), "--home", str(root),
                               "--dry-run", "--json"], data_root)
        rust = run_json(RUST_BIN, ["repair", "--root", str(root), "--home", str(root),
                                   "--dry-run", "--json"], data_root)
        golden = json.loads((GOLDEN / f"{kind}.plan.json").read_text(encoding="utf-8"))

        go_text = normalize_plan(go, root) if go else ""
        rust_text = normalize_plan(rust, root) if rust else ""
        golden_text = json.dumps(golden, ensure_ascii=False)

        verdicts = {
            "go==golden": go_text == golden_text,
            "rust==golden": rust_text == golden_text,
            "go==rust": go_text == rust_text,
        }
        ok = all(verdicts.values())
        failures += 0 if ok else 1
        print(f"{kind:16}", "PASS" if ok else "FAIL " + json.dumps(verdicts))
        if not ok:
            print("  go   :", go_text[:600])
            print("  rust :", rust_text[:600])
            print("  golden:", golden_text[:600])
    return failures


def run_json(binary, args, data_root):
    environment = dict(os.environ)
    environment["O824_DATA_ROOT"] = str(data_root)
    result = subprocess.run([str(binary), *args], capture_output=True, text=True, env=environment)
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except ValueError:
        return None


def transcript_checks():
    """Drive both mocks with each transcript's own requests and compare frames.

    This is the gate G4 check that needs no kernel: the client side is the
    recording's own requests, and the expected server side is the recording.
    """
    print("\n== transcripts ==")
    failures = 0
    for path in sorted((RS / "spec" / "transcripts").glob("*.jsonl")):
        lines = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()
                 if line.strip()]
        requests = [line for line in lines if "method" in line and "id" in line]
        recorded = [line for line in lines if not ("method" in line and "id" in line)]

        def frame(document):
            body = json.dumps(document, separators=(",", ":")).encode("utf-8")
            return b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body

        def drive(binary):
            payload = b"".join(frame(request) for request in requests)
            result = subprocess.run(
                [str(binary), "mock", "--transcript", str(path)],
                input=payload, capture_output=True,
            )
            if result.returncode != 0:
                return None
            frames, position = [], 0
            while position < len(result.stdout):
                head, _, rest = result.stdout[position:].partition(b"\r\n\r\n")
                header = head.decode("ascii", "replace")
                length = int(header.split(":", 1)[1].strip())
                frames.append(json.loads(rest[:length]))
                position += len(head) + 4 + length
            return frames

        go = drive(GO_BIN)
        rust = drive(RUST_BIN)
        ok = go == recorded and rust == recorded and go == rust
        failures += 0 if ok else 1
        print(f"{path.name:24}", "PASS" if ok else "FAIL")
        if not ok:
            print("  go    :", json.dumps(go)[:500] if go else "no output")
            print("  rust  :", json.dumps(rust)[:500] if rust else "no output")
            print("  record:", json.dumps(recorded)[:500])
    return failures


def machine_checks():
    """Compare the two implementations against the real machine, not a fixture.

    This is the half the in-repo tests cannot do: the fixture trees never exercise
    the uninstall registry or the fixed-volume scan, and `--install` only works
    when the machine really has an installation. On a machine with no DSH install
    the check says so instead of failing.
    """
    print("\n== real machine ==")
    failures = 0

    go_code, go_json = run_simple(GO_BIN, ["discover", "--json"])
    rs_code, rs_json = run_simple(RUST_BIN, ["discover", "--json"])

    # The reference implementation knows one shape: the desktop application. The Rust
    # side also finds the web form (a global npm package) and numbers installations by
    # path, so the comparison is over the desktop installations and ignores the ids,
    # which are per-machine labels rather than content.
    def desktops(text):
        try:
            found = [install for install in json.loads(text) if install.get("kind") == "desktop"]
        except ValueError:
            return None
        for install in found:
            install.pop("installId", None)
        return found

    go_desktops, rs_desktops = desktops(go_json), desktops(rs_json)
    if go_code != rs_code or go_desktops is None or go_desktops != rs_desktops:
        failures += 1
        print("discover --json      FAIL")
        print("  go   :", go_json[:400], f"(exit {go_code})")
        print("  rust :", rs_json[:400], f"(exit {rs_code})")
    else:
        print("discover --json      PASS")

    go_code, go_out = run_simple(GO_BIN, ["discover"])
    rs_code, rs_out = run_simple(RUST_BIN, ["discover"])
    # The table is compared row for row, but only the desktop rows exist on both sides;
    # the id column shifts once a web installation sorts before the desktop one.
    def desktop_rows(text):
        return [line for line in text.splitlines() if "desktop" in line]

    if go_code != rs_code or desktop_rows(go_out) != desktop_rows(rs_out):
        failures += 1
        print("discover (table)     FAIL")
        print("  go   :", go_out[:600])
        print("  rust :", rs_out[:600])
    else:
        print("discover (table)     PASS")

    try:
        installs = json.loads(go_json)
    except ValueError:
        installs = []
    if not installs:
        print("doctor --install     SKIP (no installation on this machine)")
    else:
        # The desktop installation on each side, by id.
        desktop_id = next(
            (install["installId"] for install in installs if install.get("kind") == "desktop"),
            installs[0]["installId"],
        )
        rust_installs = json.loads(rs_json) if rs_json else []
        rust_desktop_id = next(
            (install["installId"] for install in rust_installs if install.get("kind") == "desktop"),
            desktop_id,
        )
        install_id = desktop_id
        go_code, go_out = run_simple(GO_BIN, ["doctor", "--install", desktop_id, "--json"])
        rs_code, rs_out = run_simple(RUST_BIN, ["doctor", "--install", rust_desktop_id, "--json"])

        # P1 added checks the reference implementation never had. They are named here so
        # the divergence is a statement rather than a surprise: everything else must
        # still match byte for byte, and the added check must be present and correct.
        ADDED = {"F11"}

        def without_added(text):
            try:
                envelope = json.loads(text)
            except ValueError:
                return text
            envelope["findings"] = [
                finding for finding in envelope.get("findings", [])
                if finding.get("checkId") not in ADDED
            ]
            return json.dumps(envelope, sort_keys=True)

        ok = without_added(go_out) == without_added(rs_out)
        failures += 0 if ok else 1
        print(f"doctor --install {install_id:<3} {'PASS' if ok else 'FAIL'}")
        if not ok:
            print("  go   :", go_out[:400], f"(exit {go_code})")
            print("  rust :", rs_out[:400], f"(exit {rs_code})")

        # And the added check has to actually say the right thing on this machine.
        try:
            findings = json.loads(rs_out).get("findings", [])
        except ValueError:
            findings = []
        drift = next((f for f in findings if f.get("checkId") == "F11"), None)
        versions = {
            install["kind"]: install["version"]
            for install in json.loads(rs_json)
            if install.get("kind") in ("desktop", "web") and install.get("version")
        }
        expected = len(set(versions.values())) > 1
        correct = (drift is not None) == expected and (
            drift is None or drift.get("params", {}).get("versions") == versions
        )
        failures += 0 if correct else 1
        print(f"F11 version drift   {'PASS' if correct else 'FAIL'}"
              f"{'' if expected else ' (forms agree, so none expected)'}")
        if not correct:
            print("  versions:", versions, "finding:", drift)

    return failures


def run_simple(binary, args):
    """Runs a command and returns its exit code and combined stdout."""
    result = subprocess.run([str(binary), *args], capture_output=True, text=True)
    return result.returncode, result.stdout


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

    print(f"\n{len(KINDS) - failures}/{len(KINDS)} doctor kinds agree")

    failures += plan_checks()
    failures += transcript_checks()
    failures += machine_checks()

    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
