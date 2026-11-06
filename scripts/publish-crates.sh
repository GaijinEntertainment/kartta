#!/usr/bin/env bash
set -euo pipefail

readonly INDEX=https://index.crates.io

usage() {
  cat <<'USAGE'
usage: scripts/publish-crates.sh            publish every crate whose `publish` is unset and whose version crates.io
                                            lacks, in dependency order
       scripts/publish-crates.sh --dry-run  package and verify every such crate, and upload nothing
USAGE
}

# Prints each crate of the publish set whose version the sparse index already lists, yanked or not.
held_crates() {
  local crates name version response status
  crates=$(cargo metadata --locked --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.publish == null) | "\(.name) \(.version)"') || return
  while read -r name version; do
    # constraint: the index files a name of four characters or more under its first two, then its next two
    response=$(curl -sS --retry 3 -w '\n%{http_code}' -- "$INDEX/${name:0:2}/${name:2:2}/$name") || return
    status=${response##*$'\n'}
    case "$status" in
      200)
        if jq -en --arg version "$version" '[inputs.vers] | index($version) != null' <<<"${response%$'\n'*}" \
          >/dev/null; then
          printf '%s\n' "$name"
        fi
        ;;
      404 | 410 | 451) ;;
      *)
        printf '%s: the index answered HTTP %s\n' "$name" "$status" >&2
        return 1
        ;;
    esac
  done <<<"$crates"
}

main() {
  cd "$(dirname "${BASH_SOURCE[0]}")/.."
  local dry_run=false held name
  local args=(--workspace --locked)
  case "$#:${1-}" in
    0:) ;;
    1:--dry-run) dry_run=true ;;
    1:-h | 1:--help)
      usage
      return
      ;;
    *)
      usage >&2
      return 64
      ;;
  esac
  held=$(held_crates)
  if [[ -n "$held" ]]; then
    printf 'crates.io holds this version of %s\n' "${held//$'\n'/, }"
  fi
  if [[ "$dry_run" == true ]]; then
    # A dry run checks this tree, which the registry's copy of the same version need not match, so it skips no crate.
    cargo publish "${args[@]}" --dry-run
    return
  fi
  for name in $held; do
    args+=(--exclude "$name")
  done
  cargo publish "${args[@]}"
}

main "$@"
