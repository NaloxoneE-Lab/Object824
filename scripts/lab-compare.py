"""Compare the two implementations on the real-machine lab.

For every scenario this script rebuilds the broken tree, runs
`doctor -> repair --yes -> doctor` with the Go oracle, captures the exit codes,
the output and a digest of the tree, then rebuilds the same scenario and does the
same with the Rust binary, and finally compares all of it.

Two things are normalized rather than compared literally:

* the plan id, which contains the instant the plan was built;
* YAML file contents, which are compared *semantically*. The reference
  implementation re-emits a YAML tree and this one edits lines in place, so the
  same repair can produce different bytes for the same document — the M0 spike
  chose that trade deliberately (see docs/week1/yaml-spike-result.md). Comparing
  the parsed document is the honest test of "same file effect".

Usage (from o824-rs):

    cargo build --release --bin o824
    python scripts/lab-compare.py [--scenario f01 ...]
"""

import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys

import yaml

RS = pathlib.Path(__file__).resolve().parent.parent
REPO = RS.parent
LAB = REPO / ".." / "broken-dsh-lab"
GO_BIN = REPO / "o824.exe"
RUST_BIN = RS / "target" / "release" / "o824.exe"
SCENARIOS = ["f03", "f01", "f02", "f16", "f17", "f20"]
PLAN_ID = re.compile(r"plan-\d{8}T\d{6}Z-[0-9a-f]{8}")


def build(scenario):
    subprocess.run(
        ["pwsh", "-NoProfile", "-ExecutionPolicy", "Bypass",
         "-File", str(LAB / "build.ps1"), "-Scenario", scenario],
        check=True, capture_output=True,
    )


def run(exe, args, data_root):
    environment = dict(os.environ)
    environment["O824_DATA_ROOT"] = str(data_root)
    result = subprocess.run([str(exe), *args], capture_output=True, text=True, env=environment)
    return result.returncode, result.stdout.strip(), result.stderr.strip()


def walk_files(root):
    """Every regular file under root, never following a reparse point.

    The lab deliberately builds dangling junctions (F02), so a recursive glob
    would either follow one into a deleted target or raise. A link is reported by
    neither branch, which is exactly right: a link's own state is what the
    diagnosis reports, not something a file digest should read through.
    """
    stack = [root]
    while stack:
        directory = stack.pop()
        try:
            entries = list(os.scandir(directory))
        except OSError:
            continue
        for entry in entries:
            try:
                if entry.is_dir(follow_symlinks=False):
                    stack.append(pathlib.Path(entry.path))
                elif entry.is_file(follow_symlinks=False):
                    yield pathlib.Path(entry.path)
            except OSError:
                continue


def tree_digest(root):
    """A semantic digest of every settings file under root."""
    digest = {}
    if not root.is_dir():
        return digest
    for path in sorted(walk_files(root)):
        if "node_modules" in path.parts:
            continue
        rel = path.relative_to(root).as_posix()
        raw = path.read_bytes()
        if path.suffix.lower() in (".yaml", ".yml"):
            try:
                parsed = yaml.safe_load(raw.decode("utf-8"))
                digest[rel] = "yaml:" + json.dumps(parsed, sort_keys=True, default=str)
            except Exception:
                digest[rel] = "raw:" + hashlib.sha256(raw).hexdigest()
        else:
            digest[rel] = "raw:" + hashlib.sha256(raw).hexdigest()
    return digest


def invoke(scenario, exe):
    build(scenario)
    app = LAB / scenario / "app"
    home = LAB / scenario / "home"
    data = LAB / scenario / "data"

    exit1, doctor1, _ = run(exe, ["doctor", "--root", str(app), "--home", str(home)], data)
    exit_repair, repair, repair_err = run(
        exe, ["repair", "--root", str(app), "--home", str(home), "--yes"], data)
    exit2, doctor2, _ = run(exe, ["doctor", "--root", str(app), "--home", str(home)], data)

    return {
        "exit1": exit1,
        "doctor1": PLAN_ID.sub("<PLAN>", doctor1),
        "exit_repair": exit_repair,
        "repair": PLAN_ID.sub("<PLAN>", repair),
        "repair_err": PLAN_ID.sub("<PLAN>", repair_err),
        "exit2": exit2,
        "doctor2": PLAN_ID.sub("<PLAN>", doctor2),
        "app": tree_digest(app),
        "home": tree_digest(home),
    }


def compare(name, go, rust):
    problems = []
    for field in ("exit1", "doctor1", "exit_repair", "repair", "repair_err", "exit2", "doctor2"):
        if go[field] != rust[field]:
            problems.append(field)
    for field in ("app", "home"):
        if go[field] != rust[field]:
            differing = sorted(
                key for key in set(go[field]) | set(rust[field])
                if go[field].get(key) != rust[field].get(key))
            problems.append(f"{field}: {differing}")
    return problems


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", action="append", dest="scenarios")
    arguments = parser.parse_args()
    scenarios = arguments.scenarios or SCENARIOS

    for required in (GO_BIN, RUST_BIN):
        if not required.is_file():
            print(f"missing {required}; build it first")
            return 2

    failures = 0
    for scenario in scenarios:
        go = invoke(scenario, GO_BIN)
        rust = invoke(scenario, RUST_BIN)
        problems = compare(scenario, go, rust)
        if not problems:
            print(f"{scenario:6} PASS  doctor {go['exit1']}/{rust['exit1']}  "
                  f"repair {go['exit_repair']}  doctor {go['exit2']}/{rust['exit2']}")
            continue
        failures += 1
        print(f"{scenario:6} FAIL  {problems}")
        for field in ("doctor1", "repair", "repair_err", "doctor2"):
            if field in problems:
                print(f"  --- go {field} ---\n{go[field]}")
                print(f"  --- rust {field} ---\n{rust[field]}")
        for field in ("app", "home"):
            for key in sorted(set(go[field]) | set(rust[field])):
                if go[field].get(key) != rust[field].get(key):
                    print(f"  {field}/{key}\n    go  : {go[field].get(key)}")
                    print(f"    rust: {rust[field].get(key)}")

    print(f"\n{len(scenarios) - failures}/{len(scenarios)} scenarios agree")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
