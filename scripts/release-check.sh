#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
usage: scripts/release-check.sh fetch <version> <dir> <artifact>...
       scripts/release-check.sh mcp <version>
       scripts/release-check.sh crate <version>
       scripts/release-check.sh go <version> <tarball>
       scripts/release-check.sh npm <version> [<dir>]
       scripts/release-check.sh sums <dir>

The install checks of the release <version>, from the public registries and the GitHub release `v<version>`. With
RELEASE_RUN set to the id of a Release workflow run, that run's artifacts stand in for the GitHub release and npm, and
this checkout stands in for crates.io and the Go module proxy.

  fetch  downloads each artifact into <dir>, named as the Release workflow names it: a target's tarball, SHA256SUMS,
         or `npm-<package>`, which only a run serves
  mcp    installs kartta-mcp with `cargo install --locked`, and checks that `--version` names <version>
  crate  builds a scratch crate that depends on kartta at exactly <version>
  go     runs a scratch Go program that requires the module at v<version>, linked against the archive in <tarball>,
         and checks the version the library answers
  npm    installs @gaijin/kartta at <version>, or every pack in <dir> under RELEASE_RUN, and checks the version the
         engine answers
  sums   checks every tarball in <dir> against the SHA256SUMS there, and that the file lists no other
USAGE
}

fail() {
  printf 'release-check: %s\n' "$*" >&2
  exit 1
}

# A path as a native Windows program reads it, inside a file or a variable, where Git's bash does not rewrite it.
native() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -m "$1"
  else
    printf '%s\n' "$1"
  fi
}

absolute() {
  printf '%s/%s\n' "$(cd "$(dirname "$1")" && pwd)" "$(basename "$1")"
}

fetch() {
  version=$1 dir=$2
  shift 2
  mkdir -p "$dir"
  for artifact in "$@"; do
    if [ -n "${RELEASE_RUN-}" ]; then
      gh run download "$RELEASE_RUN" --name "$artifact" --dir "$dir"
      continue
    fi
    case $artifact in
      npm-*) ;;
      SHA256SUMS) gh release download "v$version" --pattern SHA256SUMS --dir "$dir" ;;
      *) gh release download "v$version" --pattern "kartta-ffi-$version-$artifact.tar.gz" --dir "$dir" ;;
    esac
  done
}

check_mcp() {
  version=$1
  bin=$scratch/bin
  if [ -n "${RELEASE_RUN-}" ]; then
    cargo install --locked --root "$bin" --path "$root/crates/kartta-mcp"
  else
    cargo install --locked --root "$bin" --version "$version" kartta-mcp
  fi
  printed=$("$bin/bin/kartta-mcp" --version)
  [ "$printed" = "kartta-mcp $version" ] || fail "kartta-mcp --version printed '$printed'"
  printf '%s\n' "$printed"
}

check_crate() {
  version=$1
  crate=$scratch/crate
  if [ -n "${RELEASE_RUN-}" ]; then
    dependency="{ path = \"$(native "$root/crates/kartta")\", version = \"=$version\" }"
  else
    dependency="\"=$version\""
  fi
  mkdir -p "$crate/src"
  cat >"$crate/Cargo.toml" <<TOML
[package]
name = "consumer"
version = "0.1.0"
edition = "2024"

[dependencies]
kartta = $dependency
TOML
  # The engine's type, which the 0.0.0 placeholder on crates.io does not hold.
  cat >"$crate/src/main.rs" <<'RUST'
fn main() {
    println!("{}", std::any::type_name::<kartta::Engine>());
}
RUST
  (cd "$crate" && cargo build)
}

check_go() {
  version=$1
  tarball=$(absolute "$2")
  program=$scratch/go
  mkdir "$program"
  tar -xzf "$tarball" -C "$program"
  library=$program/$(basename "$tarball" .tar.gz)
  [ -d "$library" ] || fail "$tarball holds no $(basename "$library")"
  cat >"$program/main.go" <<'GO'
package main

import (
	"fmt"
	"os"

	"dev.gaijin.team/go/kartta"
)

func main() {
	k, err := kartta.Open(".")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	version, err := k.Version()
	k.Close()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	fmt.Println(version.Version)
}
GO
  cd "$program"
  go mod init consumer
  if [ -n "${RELEASE_RUN-}" ]; then
    go mod edit -require="dev.gaijin.team/go/kartta@v$version" \
      -replace="dev.gaijin.team/go/kartta=$(native "$root/bindings/go")"
  else
    go get "dev.gaijin.team/go/kartta@v$version"
  fi
  answered=$(CGO_LDFLAGS="-L$(native "$library")" go run .)
  [ "$answered" = "$version" ] || fail "the Go binding's library answered version '$answered'"
  printf 'the Go binding answered %s\n' "$answered"
}

check_npm() {
  version=$1
  packs=
  if [ -n "${RELEASE_RUN-}" ]; then
    packs=$(cd "${2:?RELEASE_RUN needs the directory of the packs}" && pwd)
  fi
  project=$scratch/npm
  mkdir "$project"
  cd "$project"
  npm init -y >/dev/null
  if [ -n "$packs" ]; then
    npm install --no-audit --no-fund "$packs"/*.tgz
  else
    npm install --no-audit --no-fund "@gaijin/kartta@$version"
  fi
  cat >version.mjs <<'JS'
import {Kartta} from '@gaijin/kartta';

const kartta = Kartta.open('.');
try {
	console.log((await kartta.version()).version);
} finally {
	kartta.close();
}
JS
  answered=$(node version.mjs)
  [ "$answered" = "$version" ] || fail "the npm package's engine answered version '$answered'"
  printf 'the npm package answered %s\n' "$answered"
}

check_sums() {
  cd "$1"
  sha256sum --check --strict SHA256SUMS
  [ "$(awk '{ print $2 }' SHA256SUMS | sort)" = "$(printf '%s\n' *.tar.gz | sort)" ] ||
    fail "SHA256SUMS does not list exactly the tarballs of $1"
}

root=$(cd "$(dirname "$0")/.." && pwd)
# The scratch crates sit outside the checkout, where its toolchain pin does not reach.
RUSTUP_TOOLCHAIN=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$root/rust-toolchain.toml")
export RUSTUP_TOOLCHAIN
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

case ${1-}:$# in
  fetch:[4-9] | fetch:[1-9][0-9])
    shift
    fetch "$@"
    ;;
  mcp:2) check_mcp "$2" ;;
  crate:2) check_crate "$2" ;;
  go:3) check_go "$2" "$3" ;;
  npm:2 | npm:3) check_npm "$2" "${3-}" ;;
  sums:2) check_sums "$2" ;;
  -h:1 | --help:1) usage ;;
  *)
    usage >&2
    exit 64
    ;;
esac
