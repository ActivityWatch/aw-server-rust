#!/bin/bash

set -euo pipefail

REPO_ROOT="$(
    unset CDPATH
    cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
    pwd
)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

NDK_BIN="$TMPDIR/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
mkdir -p "$NDK_BIN" "$TMPDIR/ndk/sysroot/lib" "$TMPDIR/bin"
touch "$TMPDIR/ndk/sysroot/lib/libunwind.a" "$TMPDIR/libgcc.a"

cat > "$NDK_BIN/llvm-ar" <<'EOF'
#!/bin/bash
exit 0
EOF

cat > "$NDK_BIN/llvm-ranlib" <<'EOF'
#!/bin/bash
exit 0
EOF

cat > "$NDK_BIN/aarch64-linux-android-clang" <<EOF
#!/bin/bash
if [ "\$1" = "-print-libgcc-file-name" ]; then
    echo "$TMPDIR/libgcc.a"
fi
exit 0
EOF

cat > "$TMPDIR/bin/cargo" <<EOF
#!/bin/bash
printf '%s\n' "\$PWD" >> "$TMPDIR/cargo_pwds.txt"
printf '%s\n' "\$*" >> "$TMPDIR/cargo_args.txt"
exit 0
EOF

chmod +x \
    "$NDK_BIN/llvm-ar" \
    "$NDK_BIN/llvm-ranlib" \
    "$NDK_BIN/aarch64-linux-android-clang" \
    "$TMPDIR/bin/cargo"

(
    cd /tmp
    PATH="$TMPDIR/bin:$PATH" \
    ANDROID_NDK_HOME="$TMPDIR/ndk" \
    RELEASE=false \
    "$REPO_ROOT/compile-android.sh" arm64 >/dev/null
)

if [ ! -s "$TMPDIR/cargo_pwds.txt" ]; then
    echo "Expected cargo to be invoked"
    exit 1
fi

if [ "$(wc -l < "$TMPDIR/cargo_pwds.txt")" -ne 2 ]; then
    echo "Expected exactly two cargo invocations"
    cat "$TMPDIR/cargo_args.txt"
    exit 1
fi

if grep -Fxv "$REPO_ROOT" "$TMPDIR/cargo_pwds.txt" >/dev/null; then
    echo "cargo ran outside repo root:"
    cat "$TMPDIR/cargo_pwds.txt"
    exit 1
fi

grep -F -- "-p aw-server" "$TMPDIR/cargo_args.txt" >/dev/null
grep -F -- "-p aw-sync" "$TMPDIR/cargo_args.txt" >/dev/null
