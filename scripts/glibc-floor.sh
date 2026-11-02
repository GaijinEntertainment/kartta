#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/glibc-floor.sh <target> <floor> <kartta-mcp.tar.gz> <kartta-ffi.tar.gz> <out dir>

Run inside the release's Linux gnu build image. Fails when kartta-mcp asks for a glibc past <floor> or the archive does
not link there, and leaves kartta-mcp and the linked kartta-abi in <out dir>, for a host at the floor to run.
USAGE
}

if [ "$#" -ne 5 ]; then
  usage >&2
  exit 64
fi
target=$1
floor=$2
out=$5

mkdir -p "$out"
tar -xzf "$3" -C "$out" --strip-components=1
tar -xzf "$4" -C "$out" --strip-components=1

highest=$(objdump -T "$out/kartta-mcp" | grep -o 'GLIBC_[0-9.]*' | cut -d_ -f2 | sort -V | tail -n 1)
if [ "$(printf '%s\n%s\n' "$floor" "$highest" | sort -V | tail -n 1)" != "$floor" ]; then
  printf 'kartta-mcp needs glibc %s, past the floor %s\n' "$highest" "$floor" >&2
  exit 1
fi
printf 'kartta-mcp needs glibc %s at most\n' "$highest"

"$(dirname "$0")/link-check.sh" "$target" "$out"
mv "$out/a.out" "$out/kartta-abi"
