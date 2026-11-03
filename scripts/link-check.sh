#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/link-check.sh <target> <directory>   link and run a C program as WIRE.md states for <target>

<directory> holds what a kartta-ffi tarball unpacks to. Runs each command of WIRE.md's Linking table in it, prints the
archive's size and each link's time, and leaves the last program there. Every target but the MSVC one links each
command again beside a C file that defines a tree-sitter name and a grammar's.
USAGE
}

if [ "$#" -ne 2 ]; then
  usage >&2
  exit 64
fi
target=$1
cd "$2"

case $target in
  *-windows-msvc) program=main.exe ;;
  *-windows-gnu) program=a.exe ;;
  *) program=a.out ;;
esac

links=$(awk -F'`' -v target="$target" '$2 == target { print $4 }' WIRE.md)
if [ -z "$links" ]; then
  printf 'WIRE.md states no link for %s\n' "$target" >&2
  exit 1
fi

printf '#include "kartta.h"\nint main(void) { return kartta_abi_version() != KARTTA_ABI_VERSION; }\n' >main.c
ls -l -- *kartta_ffi.*

printf '%s\n' "$links" | while read -r link; do
  printf '%s\n' "$link"
  time $link
  "./$program"
done

case $target in
  *-windows-msvc) ;;
  *)
    printf 'void ts_parser_new(void) {}\nvoid tree_sitter_rust(void) {}\n' >clash.c
    printf '%s\n' "$links" | while read -r link; do
      $link clash.c
      "./$program"
    done
    ;;
esac
