#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/ffi-archive.sh <target>   build the static library a release ships into <target directory>/<target>/dist

On every target but the MSVC one, the library defines the C ABI alone (ADR 4.58).
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
  *-windows-gnu)
    # constraint: a partial link merges the import sections of every DLL into one, which binds a function to another
    # DLL, so the import members stay out of it and beside the object
    objdump -h "$archive" | awk -v imports="$built/imports" -v others="$built/others" '
      function flush() { if (member != "") print member >(idata ? imports : others) }
      / file format / { flush(); member = $1; sub(/:$/, "", member); idata = 0 }
      $2 ~ /^\.idata/ { idata = 1 }
      END { flush() }'
    cp "$archive" "$built/rest.a"
    ar d "$built/rest.a" "@$built/imports"
    # shellcheck disable=SC2046,SC2086 # one flag per export
    ld -r $(printf ' -u %s' $exports) "$built/rest.a" -o "$object"
    # constraint: a partial link keeps the sections a final link drops: the grammars' exports, which name local
    # symbols now, and LLVM's bitcode, which an image would map outside itself
    objcopy --wildcard --keep-global-symbol 'kartta_*' -R .drectve -R .llvmbc -R .llvmcmd "$object"
    cp "$archive" "$built/imports.a"
    ar d "$built/imports.a" "@$built/others"
    ar rs "$built/imports.a" "$object"
    rm "$archive" "$object" "$built/rest.a" "$built/imports" "$built/others"
    mv "$built/imports.a" "$archive"
    exit 0
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
