#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/glibc-floor.sh <floor> <kartta-mcp.tar.gz> <kartta-ffi.tar.gz> <out dir>

Run inside the release's Linux gnu build image. Fails when kartta-mcp asks for a glibc past <floor> or the archive does
not link there, and leaves kartta-mcp and the linked kartta-abi in <out dir>, for a host at the floor to run.
USAGE
}

if [ "$#" -ne 4 ]; then
  usage >&2
  exit 64
fi
floor=$1
out=$4

mkdir -p "$out"
tar -xzf "$2" -C "$out" --strip-components=1
tar -xzf "$3" -C "$out" --strip-components=1

highest=$(objdump -T "$out/kartta-mcp" | grep -o 'GLIBC_[0-9.]*' | cut -d_ -f2 | sort -V | tail -n 1)
if [ "$(printf '%s\n%s\n' "$floor" "$highest" | sort -V | tail -n 1)" != "$floor" ]; then
  printf 'kartta-mcp needs glibc %s, past the floor %s\n' "$highest" "$floor" >&2
  exit 1
fi
printf 'kartta-mcp needs glibc %s at most\n' "$highest"

printf '#include "kartta.h"\nint main(void) { return kartta_abi_version() != KARTTA_ABI_VERSION; }\n' \
  | cc -I"$out" -o "$out/kartta-abi" -x c - -x none "$out/libkartta_ffi.a" -lgcc_s -lrt -lpthread -lm -ldl
