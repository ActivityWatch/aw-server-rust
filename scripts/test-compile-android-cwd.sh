#!/bin/bash

set -euo pipefail

REPO_ROOT="$(
    unset CDPATH
    cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
    pwd
)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

platform="$(uname -s | tr '[:upper:]' '[:lower:]')"
NDK_BIN="$TEST_DIR/ndk/toolchains/llvm/prebuilt/${platform}-x86_64/bin"
mkdir -p "$NDK_BIN" "$TEST_DIR/ndk/sysroot/lib" "$TEST_DIR/bin"
touch "$TEST_DIR/ndk/sysroot/lib/libunwind.a" "$TEST_DIR/libgcc.a"

cat > "$NDK_BIN/llvm-ar" <<'EOF'
#!/bin/bash
exit 0
EOF

cat > "$NDK_BIN/llvm-ranlib" <<'EOF'
#!/bin/bash
exit 0
EOF

write_clang_stub() {
    cat > "$NDK_BIN/$1" <<EOF
#!/bin/bash
if [ "\$1" = "-print-libgcc-file-name" ]; then
    echo "$TEST_DIR/libgcc.a"
fi
exit 0
EOF
}

write_clang_stub aarch64-linux-android-clang
write_clang_stub arm-linux-androideabi-clang
write_clang_stub i686-linux-android-clang
write_clang_stub x86_64-linux-android-clang

cat > "$TEST_DIR/bin/cargo" <<EOF
#!/bin/bash
printf '%s\n' "\$PWD" >> "$TEST_DIR/cargo_pwds.txt"
printf '%s\n' "\$*" >> "$TEST_DIR/cargo_args.txt"
exit 0
EOF

chmod +x \
    "$NDK_BIN/llvm-ar" \
    "$NDK_BIN/llvm-ranlib" \
    "$NDK_BIN/aarch64-linux-android-clang" \
    "$NDK_BIN/arm-linux-androideabi-clang" \
    "$NDK_BIN/i686-linux-android-clang" \
    "$NDK_BIN/x86_64-linux-android-clang" \
    "$TEST_DIR/bin/cargo"

run_compile() {
    cd /tmp
    PATH="$TEST_DIR/bin:$PATH" \
    ANDROID_NDK_HOME="$TEST_DIR/ndk" \
    RELEASE=false \
    "$REPO_ROOT/compile-android.sh" "$@" >/dev/null
}

assert_cargo_invocations() {
    if [ ! -s "$TEST_DIR/cargo_pwds.txt" ]; then
        echo "Expected cargo to be invoked"
        exit 1
    fi

    if [ "$(wc -l < "$TEST_DIR/cargo_pwds.txt")" -ne "$1" ]; then
        echo "Expected exactly $1 cargo invocations"
        cat "$TEST_DIR/cargo_args.txt"
        exit 1
    fi

    if grep -Fxv "$REPO_ROOT" "$TEST_DIR/cargo_pwds.txt" >/dev/null; then
        echo "cargo ran outside repo root:"
        cat "$TEST_DIR/cargo_pwds.txt"
        exit 1
    fi
}

run_compile arm64
assert_cargo_invocations 2
grep -F -- "-p aw-server" "$TEST_DIR/cargo_args.txt" >/dev/null
grep -F -- "-p aw-sync" "$TEST_DIR/cargo_args.txt" >/dev/null
grep -F -- "--target aarch64-linux-android" "$TEST_DIR/cargo_args.txt" >/dev/null
if grep -F -- "--target armv7-linux-androideabi" "$TEST_DIR/cargo_args.txt" >/dev/null; then
    echo "arm64 should not match the arm target"
    exit 1
fi

: > "$TEST_DIR/cargo_pwds.txt"
: > "$TEST_DIR/cargo_args.txt"

run_compile arm
assert_cargo_invocations 2
grep -F -- "--target armv7-linux-androideabi" "$TEST_DIR/cargo_args.txt" >/dev/null
