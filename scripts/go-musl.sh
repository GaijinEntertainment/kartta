#!/bin/sh
set -eu

# Builds the archive a release ships for the musl target named, and runs the Go binding's tests against it. Runs inside
# the image of the `go-musl` job of .github/workflows/ci.yml, with a Go toolchain mounted at /usr/local/go.
target=$1

scripts/ffi-archive.sh "$target"

built=${CARGO_TARGET_DIR:-$PWD/target}/$target/dist
cp crates/kartta-ffi/include/kartta.h crates/kartta-ffi/WIRE.md "$built"
scripts/link-check.sh "$target" "$built"

export CGO_LDFLAGS="-L$built"
export PATH="/usr/local/go/bin:$PATH"
cd bindings/go

go test -race ./...
go test -ldflags=-extldflags=-static ./...
