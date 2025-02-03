import argparse
import sysconfig
import shutil
import os
import sys

def add_rust_prefix(p):
    base, ext = os.path.splitext(p)
    return f"{base}-rust{ext}"

def is_executable(p):
    """ Cross-platform check for executable files. """
    if os.path.isfile(p) and os.access(p, os.X_OK):
        return True
    return os.name == "nt" and p.lower().endswith((".exe", ".bat", ".cmd", ".com"))

def build(target_dir, python_bin_dir):

    if not os.path.exists(target_dir):
        print(f"Error: {target_dir} does not exist. Did you run `cargo build --release`?", file=sys.stderr)
        sys.exit(1)

    os.makedirs(python_bin_dir, exist_ok=True)

    for file_name in os.listdir(target_dir):
        src_file = os.path.join(target_dir, file_name)
        dst_file = add_rust_prefix(os.path.join(python_bin_dir, file_name))

        if is_executable(src_file):
            shutil.copy(src_file, dst_file)

def clean(target_dir, python_bin_dir):
    if not os.path.exists(python_bin_dir) or not os.path.exists(target_dir):
        return

    for file_name in os.listdir(target_dir):
        dst_file = add_rust_prefix(os.path.join(python_bin_dir, file_name))
        if is_executable(dst_file):
            os.remove(dst_file)

def main(args):

    python_bin_dir = sysconfig.get_path("scripts")

    if not python_bin_dir:
        python_bin_dir = os.path.dirname(sys.executable)

    if args.clean:
        clean(args.target_dir, python_bin_dir)
    else:
        build(args.target_dir, python_bin_dir)

def parse_args():
    parser = argparse.ArgumentParser()

    parser.add_argument("--clean", action='store_true', default=False)
    parser.add_argument("target_dir", type=str, default=os.path.join("target", "release"), nargs='?')

    return parser.parse_args()

if __name__ == "__main__":
    args = parse_args()
    main(args)
