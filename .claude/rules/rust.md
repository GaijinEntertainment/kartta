---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
  - "rust-toolchain.toml"
---

<rust>

- Invoke the `rust:rust` skill before writing Rust. Invoke the `rust:rust-testing` skill before writing a test.
- Follow the skills for each question that this file does not answer. Where a skill and this file differ, follow this
  file.

<lints>

- `clippy.toml` sets strict limits on argument count, boolean parameters, function length, cognitive complexity, and
  nesting depth. Read it before you design a function signature.

</lints>

<testing>

- `std::assert_matches` is unstable through Rust 1.99, and its import does not resolve. Write
  `assert!(matches!(value, Pattern))`. Do not write a `match` that ends in `panic!`.

</testing>

<versions>

- `rust-toolchain.toml` pins the toolchain for local builds and for CI. Change it in a commit of its own, and never name
  a moving channel such as `stable` in place of a version.
- Write each dependency requirement as the full version that `Cargo.lock` resolves, such as `rayon = "1.12.0"`. Never
  write an `=` requirement: these crates are published libraries, and an exact pin stops Cargo from unifying the
  requirement with a consumer's own.

</versions>

<verification>

- Run these gates before you report a change as done:
  - `cargo fmt --all`
  - `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps`
  - `cargo test --locked --workspace`
  - `cargo deny check`, after a change to the dependencies
- Run tests with `cargo test`, not `cargo nextest run`.

</verification>

</rust>
