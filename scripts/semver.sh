#!/usr/bin/env bash
set -euo pipefail

# Compares every crate's public API with the newest release tag, and passes while the repository has no such tag.
# constraint: the Go module's tags are `bindings/go/v*`, and an `-rc` tag is no release to hold an API to
baseline=$(git tag --list --sort=-version:refname | grep -Em1 '^v[0-9]+\.[0-9]+\.[0-9]+$' || true)
if [[ -z "$baseline" ]]; then
  printf 'no release tag, so no baseline to compare\n'
  exit 0
fi

args=(--workspace --all-features --baseline-rev "$baseline")

# constraint: cargo-semver-checks refuses a whole `--workspace` run over a crate that the baseline does not hold
for manifest in crates/*/Cargo.toml; do
  git cat-file -e "$baseline:$manifest" 2>/dev/null || args+=(--exclude "$(basename "$(dirname "$manifest")")")
done

# Between two cuts the workspace still holds the tag's version, and the next release is a minor one until a commit
# sets the version: an equal version would otherwise read as a patch, which refuses even a deprecation.
version=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[0].version')
if [[ "v$version" == "$baseline" ]]; then
  args+=(--release-type minor)
fi

cargo semver-checks "${args[@]}"
