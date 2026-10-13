---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
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
