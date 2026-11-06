#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/glibc-floor.sh <target> <floor> <kartta-ffi.tar.gz> <out dir> [<addon>]

Run inside the release's Linux gnu build image. Fails when the archive does not link there or the program it links asks
for a glibc past <floor>, and leaves that program in <out dir> as kartta-abi, for a host at the floor to run. Given the
Node addon, fails as well when it asks for a glibc past <floor>, and copies it into <out dir> beside the program.
USAGE
}

# Fails when the ELF file asks for a glibc past the floor.
check() {
  highest=$(objdump -T "$1" | grep -o 'GLIBC_[0-9.]*' | cut -d_ -f2 | sort -V | tail -n 1)
  if [ "$(printf '%s\n%s\n' "$floor" "$highest" | sort -V | tail -n 1)" != "$floor" ]; then
    printf '%s needs glibc %s, past the floor %s\n' "${1##*/}" "$highest" "$floor" >&2
    exit 1
  fi
  printf '%s needs glibc %s at most\n' "${1##*/}" "$highest"
}

if [ "$#" -ne 4 ] && [ "$#" -ne 5 ]; then
  usage >&2
  exit 64
fi
target=$1
floor=$2
out=$4

mkdir -p "$out"
tar -xzf "$3" -C "$out" --strip-components=1

"$(dirname "$0")/link-check.sh" "$target" "$out"
mv "$out/a.out" "$out/kartta-abi"
check "$out/kartta-abi"

if [ "$#" -eq 5 ]; then
  cp "$5" "$out"
  check "$5"
fi
