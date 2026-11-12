#!/usr/bin/env python3
"""
Strip the builder's home-directory paths from a compiled Windharp binary.

Why this exists: Rust embeds absolute source paths as panic-location metadata,
and Tauri's build script additionally writes absolute ACL permission paths into
the binary. A local build therefore bakes the builder's account name
(/Users/<name>/.cargo/registry/...) into a distributed artifact.

--remap-path-prefix removes most but not all of these (cargo does not apply
[build] rustflags to build scripts), so this replaces the remaining occurrences
in place. Replacements are the SAME LENGTH, which keeps every offset, load
command and section table valid -- the Mach-O layout is untouched.

The app is not code-signed, so there is no signature to invalidate.

Usage: scrub-paths.py <binary> [--verify]
"""
import sys

# Prefixes to neutralise, longest first so the project path wins over $HOME.
HOME = b"/Users/kevinnguyen"


def neutral(length: int) -> bytes:
    """Same length, neutral, still absolute-looking."""
    return b"/" + b"_" * (length - 1)


def scrub(path: str) -> int:
    with open(path, "rb") as fh:
        data = bytearray(fh.read())

    # Equal-length bulk replacement: offsets, load commands and section tables
    # all stay valid, so the Mach-O layout is unchanged.
    repl = neutral(len(HOME))
    total = data.count(HOME)
    if total:
        data = bytearray(bytes(data).replace(HOME, repl))
        with open(path, "wb") as fh:
            fh.write(data)
    return total


def count(path: str, needle: bytes) -> int:
    with open(path, "rb") as fh:
        return fh.read().count(needle)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)

    target = sys.argv[1]
    n = scrub(target)
    remaining = count(target, HOME)
    print(f"scrubbed {n} occurrence(s); remaining={remaining}")

    if "--verify" in sys.argv and remaining != 0:
        print("VERIFY FAILED: personal path still present", file=sys.stderr)
        sys.exit(1)
