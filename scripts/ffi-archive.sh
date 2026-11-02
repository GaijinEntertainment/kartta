#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/ffi-archive.sh <target>   build the static library a release ships into <target directory>/<target>/dist

On every target but Windows, the library defines the C ABI alone (ADR 4.58).
USAGE
}

if [ "$#" -ne 1 ]; then
  usage >&2
  exit 64
fi
case $1 in
  -h | --help)
    usage
    exit 0
    ;;
esac

target=$1
cd "$(dirname "$0")/.."

# constraint: cargo passes no `-C lto` for a crate that builds both staticlib and rlib (ADR 4.49)
cargo rustc --locked --profile dist --target "$target" -p kartta-ffi --crate-type staticlib

case $target in
  # constraint: the MSVC target's linkers make no partial link, which localizing across members needs (ADR 4.58)
  *-windows-msvc) exit 0 ;;
esac

built=${CARGO_TARGET_DIR:-target}/$target/dist
archive=$built/libkartta_ffi.a
object=$built/kartta_ffi.o

# Only what the exports reach joins the object, as only that would join a consumer's link.
exports=$(nm -g "$archive" 2>/dev/null | awk '$2 == "T" && $3 ~ /^_?kartta_/ { print $3 }' | sort -u)
case $target in
  *-apple-darwin)
    # shellcheck disable=SC2046,SC2086 # one flag per export
    cc -r -nostdlib $(printf ' -Wl,-u,%s' $exports) -Wl,-exported_symbol,'_kartta_*' "$archive" -o "$object"
    ;;
  *)
    # shellcheck disable=SC2046,SC2086 # one flag per export
    ld -r $(printf ' -u %s' $exports) "$archive" -o "$object"
    objcopy --wildcard --keep-global-symbol 'kartta_*' "$object"
    ;;
esac

# constraint: the archive is a hard link to cargo's own, which a later fresh build links here again
rm "$archive"
ar crs "$archive" "$object"
rm "$object"
