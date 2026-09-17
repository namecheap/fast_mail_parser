"""bench/Cargo.lock must resolve the same versions the wheel ships.

The bench crate's numbers are only meaningful if its dependency tree matches the
root's. It has its own lockfile (it is not a workspace member), so nothing keeps
them in step by itself -- a `cargo update` in one and not the other would have
the bench measuring a different `data-encoding` or `memchr` than the extension
links, and reporting the difference as a change in the parser.
"""
import re
import sys
from pathlib import Path


def versions(path):
    text = Path(path).read_text()
    out = {}
    for block in text.split("[[package]]"):
        name = re.search(r'^name = "(.+)"$', block, re.M)
        version = re.search(r'^version = "(.+)"$', block, re.M)
        if name and version:
            out[name.group(1)] = version.group(1)
    return out


root = versions("Cargo.lock")
bench = versions("bench/Cargo.lock")

drift = sorted(
    (name, bench[name], root[name])
    for name in bench.keys() & root.keys()
    if bench[name] != root[name]
)
if drift:
    for name, b, r in drift:
        print(f"::error::{name}: bench/Cargo.lock has {b}, Cargo.lock has {r}")
    print("::error::the bench crate would measure a different build than the wheel ships")
    sys.exit(1)

shared = len(bench.keys() & root.keys())
print(f"bench/Cargo.lock agrees with Cargo.lock on all {shared} shared packages")
