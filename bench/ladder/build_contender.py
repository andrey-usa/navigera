"""Build one compiled ladder contender and record what this build made.

    python3 bench/ladder/build_contender.py chromey

The binary is deleted first, so a binary from an earlier run (the Cargo
cache restores contenders/*/target) can never stand in for a failed build.
The record, $LADDER_BUILDS/<name>.json, holds the build's exit status, the
binary's sha256 and the contender library's version as the build resolved
it: from cargo's own artifact messages, or from the module list Go embeds
in the binary. ladder.py measures a compiled contender only when its record
says the build passed and the binary is still the one it describes.

Each contender builds from its committed lockfile (Cargo.lock: `cargo
build --locked`; go.mod + go.sum: `go build -mod=readonly`), so every run of
a commit measures the same dependency tree and a version bump is a commit.
A missing or stale lockfile fails the build with the command that refreshes
it.

Exits 0 when the build fails, so the ladder still runs the others and
reports this one's cells as "build failed"; the workflow fails the job at
its end from the records.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CONTENDERS = HERE / "contenders"
REPO = HERE.parent.parent

# What cargo and go print when the committed lockfile does not match the
# manifest (cargo --locked; go -mod=readonly).
STALE = re.compile(r"--locked was passed|lock file .* needs to be updated|cannot update the lock file"
                   r"|missing go\.sum entry|updates to go\.mod needed|go: updates to go\.sum needed"
                   r"|no required module provides package|-mod=readonly")


def rel(path: Path) -> str:
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def cargo_spec(crate: str) -> dict:
    d = CONTENDERS / crate
    return {
        "dir": d,
        "binary": d / "target" / "release" / f"ladder-{crate}",
        # json-render-diagnostics: artifact messages on stdout, warnings and
        # errors still rendered for the log on stderr.
        "commands": [["cargo", "build", "--release", "--locked", "--manifest-path", str(d / "Cargo.toml"),
                      "--message-format=json-render-diagnostics"]],
        "version": ("cargo", crate),
        "lockfiles": ["Cargo.lock"],
        "refresh": f"cargo update --workspace --manifest-path {rel(d / 'Cargo.toml')} "
                   f"(a version bump: cargo update -p {crate} --manifest-path {rel(d / 'Cargo.toml')}), "
                   f"then commit {rel(d / 'Cargo.lock')}",
    }


def go_spec(d: str, module: str) -> dict:
    path = CONTENDERS / d
    return {
        "dir": path,
        "binary": path / f"ladder-{d}",
        "commands": [["go", "build", "-mod=readonly", "-o", f"ladder-{d}", "."]],
        "version": ("go", module),
        "lockfiles": ["go.mod", "go.sum"],
        "refresh": f"(cd {rel(path)} && go mod tidy) (a version bump: go get {module}@<version> first), "
                   f"then commit {rel(path / 'go.mod')} and {rel(path / 'go.sum')}",
    }


SPECS = {
    "chromiumoxide": cargo_spec("chromiumoxide"),
    "chromey": cargo_spec("chromey"),
    "chromedp": go_spec("chromedp", "github.com/chromedp/chromedp"),
    "gorod": go_spec("gorod", "github.com/go-rod/rod"),
}


def cargo_version(stdout: str, crate: str) -> str | None:
    """The crate's version in this build's artifact messages (fresh or
    rebuilt); both package_id spellings cargo has used."""
    for line in stdout.splitlines():
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-artifact":
            continue
        pid = msg.get("package_id") or ""
        m = (re.search(r"#" + re.escape(crate) + r"@([^\s]+)$", pid)
             or re.match(re.escape(crate) + r" (\S+) \(", pid))
        if m:
            return m.group(1)
    return None


def go_version(binary: Path, module: str) -> str | None:
    """The module's version as linked into the binary (`go version -m`)."""
    try:
        out = subprocess.run(["go", "version", "-m", str(binary)], capture_output=True,
                             text=True, timeout=60).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    m = re.search(r"^\s*dep\s+" + re.escape(module) + r"\s+(\S+)", out, re.M)
    return m.group(1) if m else None


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def build(name: str, spec: dict, builds_dir: Path) -> dict:
    binary = Path(spec["binary"])
    binary.unlink(missing_ok=True)
    record: dict = {"name": name, "exit": 0, "binary": str(binary), "version": None}
    missing = [f for f in spec.get("lockfiles", []) if not (Path(spec["dir"]) / f).is_file()]
    if missing:
        record["exit"] = None
        record["error"] = (f"no committed {' or '.join(missing)} in {rel(Path(spec['dir']))}: "
                           f"create it with {spec['refresh']}")
        builds_dir.mkdir(parents=True, exist_ok=True)
        (builds_dir / f"{name}.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        return record
    stdout = ""
    for argv in spec["commands"]:
        print("$ " + " ".join(argv), flush=True)
        r = subprocess.run(argv, cwd=spec["dir"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        stdout += r.stdout or ""
        # Anything that is not a cargo JSON message is the tool's own output.
        for line in (r.stdout or "").splitlines():
            if not line.startswith("{"):
                print(line, flush=True)
        if r.stderr:
            sys.stderr.write(r.stderr)
            sys.stderr.flush()
        if r.returncode != 0:
            record["exit"] = r.returncode
            record["failed_command"] = " ".join(argv)
            if spec.get("refresh") and STALE.search(r.stderr or ""):
                record["error"] = (f"the committed {' / '.join(spec['lockfiles'])} is stale for this build "
                                   f"(`{record['failed_command']}` exited {record['exit']}): "
                                   f"refresh it with {spec['refresh']}")
            break
    if record["exit"] == 0 and not binary.is_file():
        record["exit"] = None
        record["error"] = "the build passed but wrote no binary"
    if record["exit"] == 0:
        record["sha256"] = sha256(binary)
        kind, lib = spec["version"]
        record["version"] = cargo_version(stdout, lib) if kind == "cargo" else go_version(binary, lib)
    builds_dir.mkdir(parents=True, exist_ok=True)
    (builds_dir / f"{name}.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    return record


def main(argv: list[str]) -> int:
    if len(argv) != 2 or argv[1] not in SPECS:
        print(f"usage: build_contender.py {{{','.join(SPECS)}}}", file=sys.stderr)
        return 2
    name = argv[1]
    builds_dir = Path(os.environ.get("LADDER_BUILDS") or HERE / "out" / "builds")
    record = build(name, SPECS[name], builds_dir)
    if record["exit"] == 0:
        print(f"{name}: built, version {record['version'] or 'unknown'}, sha256 {record['sha256'][:12]}")
    else:
        why = record.get("error") or f"`{record['failed_command']}` exited {record['exit']}"
        print(f"{name}: build failed: {why}; its cells read \"build failed\"")
        if os.environ.get("GITHUB_ACTIONS"):
            print(f"::error title={name} build failed::{why}; the ladder does not measure {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
