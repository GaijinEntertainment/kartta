---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
---

# Rust Conventions

The type system and the borrow checker are the design surface. A rejected borrow means the ownership structure is wrong
— restructure who owns and who borrows instead of reaching for `.clone()`, `Rc`, or `unsafe` to silence it. Encode
invariants in types so wrong code fails to compile. If existing code contradicts a rule below, follow the code and flag
the divergence.

## Naming

- RFC 430 casing: `UpperCamelCase` types/traits/variants, `snake_case` functions/methods/modules,
  `SCREAMING_SNAKE_CASE` consts/statics.
- Acronyms are one word: `Uuid`, `Stdin`, `Ffi` — never `UUID`, `StdIn`.
- Getters omit `get_`: `fn first(&self) -> &First`. `get` only when exactly one obvious value is gotten (`Cell::get`).
- Conversion prefixes signal cost: `as_` free borrow-to-borrow view, `to_` allocates or computes, `into_` consumes.
- Iterators are `iter` (`&T`) / `iter_mut` (`&mut T`) / `into_iter` (`T`); the returned type matches the method name
  (`into_iter` → `IntoIter`).
- Consistent verb-object order (`ParseAddrError`, matching `ParseIntError`). No weasel words — `Manager`, `Service`,
  `Factory`, `Helper`, `Util` name nothing; `Bookings`, not `BookingService`.
- Modules are `snake_case` and named by what they provide; a `Builder` is Rust's word for a factory.

## Ownership & Borrowing

- Accept the least-owning type that does the job: `&str` over `&String`, `&[T]` over `&Vec<T>`, `&T` over `T` when not
  consuming (`ptr_arg`). Return owned values; borrow in parameters.
- `.clone()` is a signal, not a fix: split the borrow, narrow the scope, use `Cow`, or restructure ownership. Adjust the
  callee to take `&self`/`&T` before cloning at the call site.
- `Rc`/`Arc` only for genuine shared ownership (a graph, a cache, a value with no single owner) — never to dodge a
  lifetime annotation. `Rc::clone(&x)` over `x.clone()` so the refcount bump is explicit.
- Graphs and trees are index-based arenas (`slotmap`, `generational-arena`, `petgraph`), not `Rc<RefCell<T>>` webs. When
  a cycle is unavoidable, break it with `Weak`.
- Lifetime elision wherever it applies; explicit lifetimes only when the compiler asks or when they document a real
  input-output relationship.
- Never expose `Arc<Mutex<T>>` in a public API — hide it behind `&T`/`&mut T`/`T` (`rc_mutex`, `rc_buffer`).

## Errors & Panics

- Fallible operations return `Result<T, E>`; absence-not-error returns `Option<T>`. Propagate with `?`, never
  match-and-rewrap.
- Libraries define typed errors with `thiserror`: one enum, `#[error("…")]` per variant, `#[from]` for source
  conversions. Binaries use `anyhow::Result` with `.context(…)` at each layer.
- Error types are `Debug + Display + std::error::Error` and `Send + Sync + 'static` so they cross threads and compose
  with `anyhow`/`Box<dyn Error>`.
- `.unwrap()`/`.expect()` are never error propagation: permitted on a provable invariant with
  `.expect("why this cannot fail")`, in `const` contexts, and in tests (`unwrap_used`, `expect_used`).
- A detected bug panics; a panic is not an `Error` and is never a channel for recoverable failure. Never assume a panic
  is caught.
- Invalid states are better uncompilable than validated: a `NonEmptyVec` or a parsed newtype beats a runtime check on a
  raw `Vec`/`String`.
- Typed values get newtypes or enums — a `&str` parameter for an id, kind, or path role is a stringly-typed API.

## Traits & Generics

- Static dispatch is the default: generics or `impl Trait` in argument position. `dyn Trait` only for genuine runtime
  polymorphism — heterogeneous collections, plugin boundaries, cutting deep monomorphization. Always spell `dyn`.
- Concrete type → generic → `dyn`, in that order. A generic parameter introduced for one caller is premature; generalize
  on the third use.
- No `Deref` polymorphism to fake inheritance — compose traits or forward methods explicitly. Free functions are
  first-class; not everything needs a receiver.
- Derive eagerly where semantics allow: `Debug` on every public type, plus `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`,
  `PartialOrd`, `Ord`, `Default`. `Display` for types humans read (errors, string-like wrappers).
- Conversions go through `From` (which gives `Into` free), `TryFrom`, `AsRef`, `AsMut` — not ad-hoc methods, so generic
  code and `?` interoperate.
- `impl Trait` in return position hides the concrete type; switch to a named generic when the caller must name it or
  supply it by turbofish.

## Iterators & Matching

- Express transformations as adapter chains, not index loops: `for x in &collection`, never
  `for i in 0..collection.len()` unless the index is the data (`needless_range_loop`).
- Keep chains lazy — collect once at the end (`needless_collect`); `.iter().cloned()`/`.copied()` over
  `.map(|x| x.clone())`; `.unwrap_or_else`/`.or_else` when the fallback costs anything (`or_fun_call`).
- `match` arms stay exhaustive — a trailing `_` silently swallows variants added later. Reach for the combinator when
  one exists: `opt.map(f)` over a two-arm match (`manual_map`), `it.flatten()` over `if let Some` inside a loop.
- `if let`/`while let` for single patterns; `let … else { return / continue / bail }` to bind-or-bail instead of nesting
  the happy path.
- Destructure in the pattern — function params, `let`, match arms — rather than reaching into `.0`/`.field` after.

## Concurrency — pick the model from the workload first

- CPU-bound compute (parsing, hashing, graph building) → threads or rayon `par_iter`. Async gives **zero** parallel
  speedup for CPU work.
- Many concurrent I/O operations (thousands of sockets or file handles) → async/Tokio. A handful of blocking I/O tasks →
  plain threads; an async runtime is not worth dragging in.
- Shared mutable state crosses threads as `Arc<Mutex<T>>`/`Arc<RwLock<T>>`, immutable as `Arc<T>`; move ownership with
  channels. A counter is an atomic, not a `Mutex<usize>` (`mutex_atomic`).
- Public types should be `Send` (usually `Sync`). A `!Send` value held across `.await` makes the whole future `!Send` —
  keep `Rc`/`RefCell` out of scopes that span an await.
- Never block in async: no `std::thread::sleep`, no synchronous I/O, no long CPU loops on the runtime — offload with
  `spawn_blocking` or a rayon pool.
- Deadlocks and blocking-in-async are invisible to the compiler; they are review's job, not the borrow checker's.

## unsafe

- Confined to the smallest module — soundness boundaries are module boundaries. Never a safe-looking wrapper over an
  unsound assumption.
- One `// SAFETY:` comment per `unsafe` block, one operation per block (`undocumented_unsafe_blocks`,
  `multiple_unsafe_ops_per_block`); a public `unsafe fn` documents caller invariants in a `# Safety` section.

## Modules & Manifests

- `mod.rs`-free layout: module `foo` lives in `foo.rs`, its submodules in `foo/bar.rs`. Never create a new `mod.rs`.
- Multi-crate work is one cargo workspace with shared `[workspace.dependencies]` and `[workspace.lints]`; each crate
  opts in with `[lints] workspace = true`. Split a crate when a module is independently useful.
- All lint levels live in `[lints]`/`[workspace.lints]` in `Cargo.toml` — never `#![deny(...)]` in source, never
  `RUSTFLAGS`. `#![deny(warnings)]` baked into a crate root is a defect: `-D warnings` belongs to the CI invocation.
- Narrow overrides use `#[expect(lint, reason = "…")]`, not `#[allow]` — `expect` warns once the lint stops firing, so
  stale suppressions surface. `#[allow]` is for generated code and macro expansions.
- Lint groups sit at `warn` with `priority = -1` so individual entries win; `correctness` stays at `deny`. Never enable
  the `restriction` group wholesale — opt in lint by lint (`clone_on_ref_ptr`, `undocumented_unsafe_blocks`,
  `unnecessary_safety_comment`, `multiple_unsafe_ops_per_block`, `allow_attributes_without_reason`).
- Dependencies: add only when needed, with `cargo add` so the latest compatible version is resolved — never a version
  from memory. Pre-1.0 crates are pinned to `major.minor`, their first nonzero segment being the breaking axis.
- One crate may link a given native library once: a crate setting `links = "…"` admits exactly one version of it in the
  graph, so bumping it moves every declaring crate in the same commit — a partial bump fails to resolve, not to compile.

## Docs

- `//!` documents the crate root and modules, `///` documents items. Written as part of the change; a stale doc comment
  is worse than none, so it moves in the same edit as the signature or behavior.
- Standard sections only: `# Examples`, `# Panics`, `# Errors`, `# Safety`.
- Doctests use `?` for fallible calls (wrap the body to return `Result`), never `.unwrap()` — they are the reference for
  idiomatic use. Hide setup with leading `#`, use ` ```no_run ` for examples that must compile but not run.
- Public library surfaces run under `#![warn(missing_docs)]`; link related items with intra-doc links (``[`Type`]``).

## Toolchain Gates

Not done until all three pass locally, in this order:

- `cargo fmt --all` — formatting is never hand-rolled; `--check` in CI.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — fix the cause; suppress only with a scoped
  `#[expect(…, reason = "…")]`.
- `cargo test --workspace` — includes doctests. `cargo nextest run` is a faster local runner but skips doctests, so it
  never replaces this without a paired `cargo test --doc`.

Scope with `-p <crate>` while iterating; the workspace run is what has to pass.

## Testing

Full conventions in [rust-testing.md](rust-testing.md), which loads for `tests/` and `benches/`. Unit tests live inline
in a `#[cfg(test)] mod tests` block, where no path glob can reach them — read that file before writing them.
