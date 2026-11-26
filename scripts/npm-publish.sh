#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/npm-publish.sh <dir> [<npm publish option>...]

Publishes the tarballs `npm pack` left in <dir>, at the version of bindings/node/package.json: each target's package,
then @gaijin/kartta. Skips a package whose version the registry already holds, and publishes nothing when a tarball is
missing.
USAGE
}

if [ "$#" -lt 1 ]; then
  usage >&2
  exit 64
fi
case $1 in
  -h | --help)
    usage
    exit 0
    ;;
esac

# constraint: npm publish takes a relative tarball path only when it starts with `./` (npm's npm-publish.md)
dir=$(cd "$1" && pwd)
shift
cd "$(dirname "$0")/.."

version=$(node -p 'require("./bindings/node/package.json").version')
packages=
for target in bindings/node/npm/*/; do
  target=${target%/}
  packages="$packages @gaijin/kartta-${target##*/}"
done
packages="$packages @gaijin/kartta"

# constraint: `npm pack` names the tarball of @gaijin/<name> gaijin-<name>-<version>.tgz (lib/utils/tar.js of npm/cli)
for package in $packages; do
  if [ ! -f "$dir/gaijin-${package#@gaijin/}-$version.tgz" ]; then
    printf '%s: no gaijin-%s-%s.tgz\n' "$dir" "${package#@gaijin/}" "$version" >&2
    exit 1
  fi
done

for package in $packages; do
  # The registry's `GET /{package}/{version}` (docs/REGISTRY-API.md of npm/registry) answers 200 for a version it holds,
  # and 404 for a version or a package it does not (ADR 4.65). A scoped name goes with its slash escaped, as npm's own
  # client sends it (npm-package-arg's `escapedName`).
  status=$(curl -sS --retry 3 -o /dev/null -w '%{http_code}' \
    "https://registry.npmjs.org/@gaijin%2f${package#@gaijin/}/$version")
  case $status in
    200) printf '%s@%s is on the registry: skipped\n' "$package" "$version" ;;
    404) npm publish "$dir/gaijin-${package#@gaijin/}-$version.tgz" "$@" ;;
    *)
      printf 'the registry answered %s for %s@%s\n' "$status" "$package" "$version" >&2
      exit 1
      ;;
  esac
done
