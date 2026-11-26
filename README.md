# kartta

kartta indexes a repository and answers structural questions about its code: where a symbol is defined, who calls it
and what it calls, where it is used, what implements a type, which files include a file, and what an edit reaches,
beside path and content search. One walk builds the file index and a code graph per language family, and every answer
says how much a cap or a bound left out. It answers LLM agents first, through an MCP server, and serves Rust, Go, Node
and C programs as a library.

Its languages are Go, C and C++, TypeScript, TSX, JavaScript, Rust, Python, PHP, Ruby, Lua, Bash, Puppet, daslang,
Quirrel, BLK and DSHL. TypeScript, TSX and JavaScript share one graph, every other language has its own, and no answer
follows a call from one graph into another, such as a call through FFI.

> **0.1.0 is not released yet.** Nothing is published on any channel, so none of the install lines below works today.

## Install

The MCP server builds from its crate, and Claude Code adds it for every project of the user:

```sh
cargo install --locked kartta-mcp
claude mcp add --scope user kartta -- kartta-mcp
```

Any other client launches it from `{ "command": "kartta-mcp", "args": [] }`. The toolchain the build needs, the tools
and the settings are in [the crate's README](crates/kartta-mcp/README.md).

The engine in a Rust program, with every language kartta ships:

```sh
cargo add kartta kartta-languages
```

The C library, for any other language, is one `kartta-ffi-0.1.0-<target>.tar.gz` per target on the GitHub release,
holding the static library, `kartta.h` and the wire's description ([`kartta-ffi`](crates/kartta-ffi/README.md)).

The Go module, which links the static library `libkartta_ffi.a` from the GitHub release of the same version, named in
`CGO_LDFLAGS` ([the package documentation](bindings/go/doc.go)):

```sh
go get dev.gaijin.team/go/kartta@v0.1.0
```

The npm package, for Node 24 or later, which installs the addon of its host's target
([README](bindings/node/README.md)):

```sh
npm install @gaijin/kartta
```

## Support

0.1.0 is to ship every channel for the same targets, and no artifact for a target outside them
([ADR 4.46](design-docs/adr/4.46-release-scope.md)):

- macOS on arm64.
- Linux on x64 and arm64, each with glibc from 2.28 and with musl.
- Windows on x64, with MSVC. The Go module links a `x86_64-pc-windows-gnu` archive instead, because cgo's linker
  refuses the MSVC one.

## Status

0.1.0 will be the first release: one cut of every artifact at one version. Nothing freezes at it, so a later release
may change the MCP server's tools, the C ABI and every crate's API. Each release's notes are on its GitHub release.

## Documentation

- The engine: [`kartta`](crates/kartta/README.md), and its MCP server [`kartta-mcp`](crates/kartta-mcp/README.md).
- The bindings: the C ABI [`kartta-ffi`](crates/kartta-ffi/README.md), [Go](bindings/go/doc.go) and
  [Node](bindings/node/README.md).
- The building blocks: [`kartta-walk`](crates/kartta-walk/README.md), [`kartta-fs`](crates/kartta-fs/README.md),
  [`kartta-fuzz`](crates/kartta-fuzz/README.md), [`kartta-cache`](crates/kartta-cache/README.md),
  [`kartta-graph`](crates/kartta-graph/README.md) and [`kartta-lang`](crates/kartta-lang/README.md).
- The languages: [`kartta-languages`](crates/kartta-languages/README.md), and one crate each:
  [Go](crates/kartta-lang-go/README.md), [C and C++](crates/kartta-lang-cpp/README.md),
  [TypeScript, TSX and JavaScript](crates/kartta-lang-jsts/README.md), [Rust](crates/kartta-lang-rust/README.md),
  [Python](crates/kartta-lang-python/README.md), [PHP](crates/kartta-lang-php/README.md),
  [Ruby](crates/kartta-lang-ruby/README.md), [Lua](crates/kartta-lang-lua/README.md),
  [Bash](crates/kartta-lang-bash/README.md), [Puppet](crates/kartta-lang-puppet/README.md),
  [daslang](crates/kartta-lang-daslang/README.md), [Quirrel](crates/kartta-lang-quirrel/README.md),
  [BLK](crates/kartta-lang-blk/README.md) and [DSHL](crates/kartta-lang-dshl/README.md).
- The decisions in force: [design-docs/ADR.md](design-docs/ADR.md).
- [Contributing](CONTRIBUTING.md), [releasing](RELEASING.md), [security](SECURITY.md), and the
  [MIT licence](LICENSE).
