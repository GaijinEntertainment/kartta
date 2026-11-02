#!/usr/bin/env bash
set -euo pipefail

# Replays the goldens of the newest release tag, on that tag's fixture, through this tree's wire, and passes while the
# repository has no such tag.
# constraint: the Go module's tags are `bindings/go/v*`, and an `-rc` tag is no release to hold the wire to
baseline=$(git tag --list --sort=-version:refname | grep -Em1 '^v[0-9]+\.[0-9]+\.[0-9]+$' || true)
if [[ -z "$baseline" ]]; then
  printf 'no release tag, so no wire to hold to\n'
  exit 0
fi

# A commit that sets the next major version is what lets a break land, as for the Rust API.
version=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[0].version')
released=${baseline#v}
if [[ "${version%%.*}" != "${released%%.*}" ]]; then
  printf 'heading to major %s, so %s holds this wire to nothing\n' "${version%%.*}" "$baseline"
  exit 0
fi

frozen=$(mktemp -d)
trap 'rm -rf "$frozen"' EXIT
git archive "$baseline" crates/kartta-ffi/tests/fixture crates/kartta-ffi/tests/golden | tar -x -C "$frozen"
# constraint: a stamp distrusts a file changed in the 2 seconds before a walk, and the refresh goldens pin that a
# refresh reads nothing, so a test binary already built would meet a fixture still too new
sleep 3
KARTTA_FFI_FROZEN="$frozen/crates/kartta-ffi/tests" \
  cargo test --locked -p kartta-ffi --test golden every_answer_a_release_pinned_stays
