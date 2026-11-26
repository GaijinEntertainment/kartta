#!/usr/bin/env bash
set -euo pipefail

# One site per line: a file, then the sed address of its lines that hold the version. The first is the workspace's.
# constraint: BSD sed reads `\t` as `t`, so `[[:blank:]]` stands for a tab
readonly SITES=(
  'Cargo.toml|/^\[workspace\.package\]/,/^version = /'
  'Cargo.toml|/^kartta[-a-z]* = \{ version = /'
  'Cargo.lock|/^name = "kartta(-[a-z]+)*"$/,/^version = /'
  'crates/kartta-ffi/tests/golden/version.json|/^ +"version": "/'
  'bindings/go/version.go|/^const version = "/'
  'bindings/node/package.json|/^[[:blank:]]"version": /'
  'bindings/node/package.json|/^[[:blank:]]+"@gaijin\/kartta-[a-z0-9-]+": /'
  'bindings/node/package-lock.json|/^[[:blank:]]{1,2}"name": "@gaijin\/kartta",$/,/^[[:blank:]]+"license": /'
  'bindings/node/package-lock.json|/^[[:blank:]]+"@gaijin\/kartta-[a-z0-9-]+": /'
  'bindings/node/package-lock.json|/^[[:blank:]]+"node_modules\/@gaijin\/kartta-/,/^[[:blank:]]+"version": /'
  'bindings/node/npm/darwin-arm64/package.json|/^[[:blank:]]"version": /'
  'bindings/node/npm/linux-arm64-gnu/package.json|/^[[:blank:]]"version": /'
  'bindings/node/npm/linux-arm64-musl/package.json|/^[[:blank:]]"version": /'
  'bindings/node/npm/linux-x64-gnu/package.json|/^[[:blank:]]"version": /'
  'bindings/node/npm/linux-x64-musl/package.json|/^[[:blank:]]"version": /'
  'bindings/node/npm/win32-x64-msvc/package.json|/^[[:blank:]]"version": /'
)
readonly VERSION_FIELD='(version"?|"@gaijin\/kartta-[a-z0-9-]+")( =|:) "'
readonly SEMVER='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'

usage() {
  cat <<'USAGE'
usage: scripts/version.sh <version>   set every site to <version>, then check them
       scripts/version.sh --check     fail when a site differs from the workspace version
USAGE
}

versions_at() {
  sed -En "${2}s/.*${VERSION_FIELD}([^\"]*)\".*/\\3/p" "$1"
}

check() {
  local expected site file address found differing status=0
  IFS='|' read -r file address <<<"${SITES[0]}"
  expected=$(versions_at "$file" "$address")
  for site in "${SITES[@]}"; do
    IFS='|' read -r file address <<<"$site"
    found=$(versions_at "$file" "$address" | sort -u)
    if [[ -z "$found" ]]; then
      printf '%s: no version at %s\n' "$file" "$address" >&2
      status=1
    elif differing=$(grep -vxF -- "$expected" <<<"$found"); then
      printf '%s: holds %s, the workspace %s\n' "$file" "${differing//$'\n'/, }" "$expected" >&2
      status=1
    fi
  done
  if ((status != 0)); then
    printf 'scripts/version.sh <version> sets every site\n' >&2
    return "$status"
  fi
  printf 'every site holds %s\n' "$expected"
}

set_version() {
  local site file address
  for site in "${SITES[@]}"; do
    IFS='|' read -r file address <<<"$site"
    sed -E -i.bak "${address}s/(${VERSION_FIELD})[^\"]*/\\1${1}/" "$file"
    rm -- "$file.bak"
  done
  check
}

main() {
  cd "$(dirname "${BASH_SOURCE[0]}")/.."
  if (($# != 1)); then
    usage >&2
    return 64
  fi
  case "$1" in
    -h | --help) usage ;;
    --check) check ;;
    *)
      if [[ ! "$1" =~ $SEMVER ]]; then
        usage >&2
        return 64
      fi
      set_version "$1"
      ;;
  esac
}

main "$@"
