#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/glibc-floor.sh <target> <floor> <kartta-ffi.tar.gz> <out dir>

Run inside the release's Linux gnu build image. Fails when the archive does not link there or the program it links asks
for a glibc past <floor>, and leaves that program in <out dir> as kartta-abi, for a host at the floor to run.
USAGE
}

if [ "$#" -ne 4 ]; then
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

highest=$(objdump -T "$out/kartta-abi" | grep -o 'GLIBC_[0-9.]*' | cut -d_ -f2 | sort -V | tail -n 1)
if [ "$(printf '%s\n%s\n' "$floor" "$highest" | sort -V | tail -n 1)" != "$floor" ]; then
  printf 'kartta-abi needs glibc %s, past the floor %s\n' "$highest" "$floor" >&2
  exit 1
fi
printf 'kartta-abi needs glibc %s at most\n' "$highest"
