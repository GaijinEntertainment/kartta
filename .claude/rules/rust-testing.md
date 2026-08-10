---
paths:
  - "**/tests/**/*.rs"
  - "**/benches/**/*.rs"
---

# Rust Testing

Test the public contract, not the private call graph. Real implementations and hand-written fakes beat mocks — a test
wired to a mock asserts how the code calls its collaborators, not what the code does. A reader should learn the intended
behavior from the test name and its arrange/act/assert shape.

## Layout

- Unit tests: `#[cfg(test)] mod tests` in the same file as the code under test, `use super::*` to pull the parent into
  scope. `#[cfg(test)]` keeps test code and its helpers out of the artifact.
- Private items are reachable from a child `tests` module — reserve that for genuinely tricky internals; prefer driving
  private code through the public API.
- Integration tests: top-level `tests/`, one crate per file, reaching only the public API via `use my_crate::…` exactly
  as a consumer would.
- Shared integration helpers go in `tests/common/mod.rs`, never `tests/common.rs` — the `mod.rs` form is not compiled as
  its own test crate, so it produces no spurious "running 0 tests" section. Import with `mod common;`.
- Binary crates keep `main.rs` a thin shell over `lib.rs`; only a library crate exposes items to an external `use`.
- Doctests are `///` examples on public items — compiled documentation that runs under `cargo test --doc`.
- Name a test for the behavior it pins, not the method it calls.

## Assertions

- `assert!` for booleans, `assert_eq!`/`assert_ne!` for values, with a trailing message only when the failure is not
  self-explanatory.
- `assert_matches!` (stable in `std`, `use std::assert_matches::assert_matches;`) to assert a value matches a pattern,
  including variants with bindings — never a hand-rolled `match` that ends in `panic!`.
- `pretty_assertions::{assert_eq, assert_ne}` in test modules comparing large structs or collections: line-by-line
  diffs instead of one unreadable line.
- Assert observable outcomes — return values, resulting state, emitted effects.

## Test Doubles

Cheapest double that still proves the behavior, escalating only when the boundary forces it:

1. **Real implementation** — the default for pure logic and anything without an external dependency.
2. **In-memory fake** — a working implementation backed by memory (a `HashMap`-backed store behind a trait). Preferred
   for stateful collaborators; reusable across tests.
3. **Hand-written double** — a small struct implementing the trait with canned answers, for one or two stubbed
   responses.
4. **mockall** (`#[automock]`, or `mock!` for foreign types) — only at IO or nondeterministic boundaries: network,
   filesystem, clock, randomness.

Define the collaborator behind a trait so production type and double are interchangeable. Never assert internal call
order — `Sequence`/`in_sequence` locks the call graph and breaks on harmless refactors. Argument matchers and call
counts are legitimate only when the call itself is the contract ("charged exactly once").

## Parametrization

- Plain loops or `rstest` `#[case]` for parametrized cases, `#[fixture]` for shared setup injected by parameter. Uniform
  logic only — a case needing different setup or conditional assertions is a separate test.
- `serial_test`'s `#[serial]` for tests touching shared global state: nextest runs each test in its own process, plain
  `cargo test` runs them as threads in one.
- `assert_cmd` + `predicates` to drive a compiled binary in integration tests.

## Property, Snapshot, Benchmarks

- Property tests when a function must hold an invariant across a large input space — round-trips, idempotence, ordering.
  `proptest` is the default (declarative strategies with shrinking); `quickcheck` derives generators from `Arbitrary`.
- `insta` for snapshots of large or structural output (serialized data, rendered text, debug dumps). Accept changes with
  `cargo insta review`, commit the approved `.snap` files, and keep the CLI and library on the same version.
- Benchmarks in `benches/` with `criterion`, `harness = false` on the `[[bench]]` target, inputs and outputs wrapped in
  `std::hint::black_box` so the optimizer cannot fold the work away. Benchmarks measure; they never assert correctness.

## Async

- `#[tokio::test]`, current-thread flavor by default; `flavor = "multi_thread", worker_threads = N` only when the test
  needs real parallelism.
- `tokio::time::pause()` + `advance(duration)` instead of real sleeps — virtual time fires timers instantly and takes
  wall-clock flakiness out of timeout and interval tests. Needs tokio's `test-util` feature.
- Async-trait mocks return futures via `.returning(|| Box::pin(async { … }))`; the double ladder applies unchanged.

## Determinism

- Never write to the source directory: `tempfile` for scratch space, and assert on content rather than on paths.
- No real sleeps for synchronization, no wall-clock reads, no unseeded randomness, no shared global state without
  `#[serial]` — each is a flaky-test bug, not a nuisance.
- Fixtures and corpora that tests compare against are committed; a test that regenerates its own expectation asserts
  nothing.

## Running

- `cargo test --workspace` is the gate — it runs unit, integration, and doctests.
- `cargo nextest run` is the faster local runner (process per test, cleaner isolation) but does **not** run doctests, so
  it is always paired with `cargo test --doc`.
- Selectors are shared: `-p <crate>`/`--workspace`, `--lib`/`--test <name>`, `--no-capture` for test output, nextest's
  `-E <filterset>` for expression filtering.
- `--retries` is triage, not a fix; nondeterminism gets fixed rather than retried.
- Dev-dependencies are added with `cargo add --dev` so versions resolve against the lockfile — never a version from
  memory.
