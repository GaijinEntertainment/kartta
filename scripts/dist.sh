#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/dist.sh <target>   build the release artifacts of <target> into <target directory>/artifacts

Writes kartta-mcp-<version>-<target>.tar.gz and kartta-ffi-<version>-<target>.tar.gz.
USAGE
}

# Packs the named files, with the licence and the notices, into <out>/<name>.tar.gz.
pack() {
  name=$1
  shift
  mkdir -p "$out/$name"
  cp LICENSE notices/THIRD-PARTY-NOTICES.txt "$@" "$out/$name"
  tar -czf "$out/$name.tar.gz" -C "$out" "$name"
  rm -r "${out:?}/$name"
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

case $target in
  *-windows-msvc)
    binary=kartta-mcp.exe
    archive=kartta_ffi.lib
    ;;
  *)
    binary=kartta-mcp
    archive=libkartta_ffi.a
    ;;
esac

version=$(cargo pkgid --locked -p kartta-mcp)
version=${version##*[#@]}
built=${CARGO_TARGET_DIR:-target}/$target/dist
out=${CARGO_TARGET_DIR:-target}/artifacts

cargo build --locked --profile dist --target "$target" -p kartta-mcp
# constraint: cargo passes no `-C lto` for a crate that builds both staticlib and rlib (ADR 4.49)
cargo rustc --locked --profile dist --target "$target" -p kartta-ffi --crate-type staticlib

pack "kartta-mcp-$version-$target" "$built/$binary"
pack "kartta-ffi-$version-$target" "$built/$archive" crates/kartta-ffi/include/kartta.h crates/kartta-ffi/WIRE.md
