#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/dist.sh <target>   build the release artifact of <target> into <target directory>/artifacts

Writes kartta-ffi-<version>-<target>.tar.gz, and fails when it does not link as its WIRE.md states.
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
  *-windows-msvc) archive=kartta_ffi.lib ;;
  *) archive=libkartta_ffi.a ;;
esac

version=$(cargo pkgid --locked -p kartta-ffi)
version=${version##*[#@]}
built=${CARGO_TARGET_DIR:-target}/$target/dist
out=${CARGO_TARGET_DIR:-target}/artifacts

scripts/ffi-archive.sh "$target"

pack "kartta-ffi-$version-$target" "$built/$archive" crates/kartta-ffi/include/kartta.h crates/kartta-ffi/WIRE.md

mkdir -p "$out/check"
tar -xzf "$out/kartta-ffi-$version-$target.tar.gz" -C "$out/check" --strip-components=1
scripts/link-check.sh "$target" "$out/check"
rm -r "$out/check"
