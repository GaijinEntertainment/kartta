#!/bin/sh
set -eu

# Builds the archive a release ships for the musl target named, and runs the Go binding's tests against it. Runs inside
# the image of the `go-musl` job of .github/workflows/ci.yml, with a Go toolchain mounted at /usr/local/go.
target=$1

# constraint: a debug archive overflows musl's default thread stack on a cgo thread
cargo rustc --locked --profile dist -p kartta-ffi --target "$target" --crate-type staticlib

export CGO_LDFLAGS="-L${CARGO_TARGET_DIR:-$PWD/target}/$target/dist"
export PATH="/usr/local/go/bin:$PATH"
cd bindings/go

go test -race ./...
go test -ldflags=-extldflags=-static ./...
