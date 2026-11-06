# Contributing to kartta

kartta is a code graph engine built from Rust crates, each published to crates.io, with a C ABI, a Go module and an
npm package over them. This file states how a change reaches `main`. Each rule links to where the repository keeps it,
and that source wins where the two differ.

Report a vulnerability as [`SECURITY.md`](/SECURITY.md) says, never in a public issue.

## Read the decision before the change

- Each decision in force has one record under [`design-docs/adr/`](/design-docs/adr/), and
  [`design-docs/ADR.md`](/design-docs/ADR.md) lists every record under the path it governs. Read the records of an area
  before you change how that area behaves.
- A change that conflicts with a record stops there: say so in the issue or the pull request, and do not work around
  it. Reversing a decision is the maintainers' call.
- The engineering rules every change answers to are in [`CLAUDE.md`](/CLAUDE.md): `<performance>` for time and memory,
  `<answers>` for what a query's answer carries and how a change to the graph's edges is judged, and `<constraints>`
  for the comparison against `main` and the source a claim about a language cites.

## One decision per ADR

- A record holds one decision: the rule as it stands, why it holds, what it costs, the options that lost, and what would
  reopen it. Its file is `design-docs/adr/<group>.<n>-<topic>.md`, where the topic names the question it answers.
- A change that moves a rule rewrites that record to the rule now in force and adds one line to its History. The old
  wording goes; git keeps it. A new decision takes a new record, numbered after the highest in its group, and one line
  in `design-docs/ADR.md` under the area of the first path it governs.
- An edit to a record is a commit of its own, scoped `[adr]`, after the commit whose behavior it records.

## Pinned versions

Every version the build uses is pinned, and none names `latest`, `stable` or another moving tag
([`CLAUDE.md`](/CLAUDE.md), `<constraints>`).

- **The Rust toolchain** is the version in [`rust-toolchain.toml`](/rust-toolchain.toml), which CI installs too. A
  change to it is a commit of its own ([`.claude/rules/rust.md`](/.claude/rules/rust.md)).
- **Rust dependencies** resolve through the committed `Cargo.lock`. Each cargo command that builds, checks, documents,
  tests or packages runs with `--locked`, locally and in CI, but for CI's `fresh-resolve` job, which builds the newest
  versions the requirements allow on purpose. Each requirement in a `Cargo.toml` is the full version `Cargo.lock`
  resolves, as a caret requirement, never `=` ([ADR 4.64](/design-docs/adr/4.64-dependency-requirements.md)).
- **Node dependencies** are exact in `bindings/node/package.json`, with `package-lock.json` committed and installed by
  `npm ci --ignore-scripts`, on the Node version `bindings/node/.nvmrc` names
  ([ADR 4.41](/design-docs/adr/4.41-node-toolchain.md)).
- **GitHub Actions** are pinned by their full commit SHA, with the tag in a comment beside it.
- **Container images** are pinned by digest beside their tag, and **a tool a workflow downloads** by its release, with
  the archive checked against its SHA-256, as [`.github/workflows/ci.yml`](/.github/workflows/ci.yml) does.

A pin is what the repository builds with. The floors a consumer needs are separate, and each has its own record: the
Rust floor ([ADR 4.50](/design-docs/adr/4.50-rust-floor.md)), the Go floor
([ADR 4.63](/design-docs/adr/4.63-go-floor.md)), and the glibc floor of the prebuilt artifacts
([ADR 4.55](/design-docs/adr/4.55-release-workflow.md)).

## The gates a change passes

[`.github/workflows/ci.yml`](/.github/workflows/ci.yml) runs every gate on each pull request, on Linux, macOS and
Windows, and its `ci-ok` job fails when any other job failed or was cancelled. Run the gates for what you changed before
you open the pull request.

- **Rust**, from the repository root, as [`.claude/rules/rust.md`](/.claude/rules/rust.md) lists them:
  - `cargo fmt --all`
  - `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps`
  - `cargo test --locked --workspace`
  - `cargo deny check`, after a change to the dependencies
- **Markdown**: `rumdl fmt <file>`, then `rumdl check <file>`, with the rumdl version the `markdown` job of `ci.yml`
  names, installed by `cargo install --locked rumdl@<version>`
  ([`.claude/rules/markdown.md`](/.claude/rules/markdown.md)).
- **The Go binding**: build the library with `cargo build --locked -p kartta-ffi`, then in `bindings/go`, with
  `CGO_LDFLAGS=-L<repository>/target/debug`, run `go vet ./...`, `go test -race ./...` and the `govulncheck` version the
  `go` job of `ci.yml` names.
- **The Node binding**: in `bindings/node`, `npm ci --ignore-scripts`, then `vp fmt --check`, `vp lint`,
  `vp run typecheck`, `vp run build` and `vp test`, with the `vp` of `node_modules/.bin`
  ([`.claude/rules/node.md`](/.claude/rules/node.md)).
- **A dependency or toolchain bump**: `python3 notices/generate.py`, and commit the `notices/THIRD-PARTY-NOTICES.txt` it
  writes.
- **A version change**: `scripts/version.sh --check`.

A pull request that touches a path the `pull_request` filter of
[`.github/workflows/release.yml`](/.github/workflows/release.yml) lists, such as a Cargo manifest, `Cargo.lock` or the
toolchain pin, also runs the release workflow's builds and publishes as dry runs; a change to crate source alone runs
none of them ([ADR 4.55](/design-docs/adr/4.55-release-workflow.md)). They report and do not gate.

## Review and landing

- `main` takes a change only through a pull request. Its ruleset requires a linear history and a passing `ci-ok`, and
  the maintainers merge by rebase, so each commit lands on `main` as you wrote it.
- So each commit holds one kind of work and builds and passes on its own. Its subject is the imperative, after the
  scope it touches in brackets: a crate, `ci`, `adr` or `workspace`, as `git log` shows.
- A maintainer reviews each commit on its own, and a commit lands only once its final version has been reviewed. Fold a
  fix that review asks for into the commit it corrects, and push the branch again.
- A pull request lands once every commit has passed review and `ci-ok` passes.
