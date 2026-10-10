#!/usr/bin/env python3

import os
import sys
import subprocess

def find_rust_binary():
    """Finds the Rust binary in the target directory."""

    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    target = "debug" if os.getenv("RELEASE", "") == "false" else "release"

    rust_binary = os.path.join(repo_root, "target", target, "aw-server")

    if os.path.exists(rust_binary):
        return rust_binary

    print(f"Error: Rust binary '{rust_binary}' not found. Did you run `cargo build --release`?", file=sys.stderr)
    sys.exit(1)

def main():
    """Executes the Rust binary and forwards all arguments."""
    rust_binary = find_rust_binary()
    if os.name == "posix":
        # Replace current Python process with rust binary
        os.execvp(rust_binary, (rust_binary, *sys.argv[1:]))
    else:
        subprocess.run([rust_binary] + sys.argv[1:])

if __name__ == "__main__":
    main()
