//! C ABI of the kartta code graph engine: a handle on a tree, one JSON call per query, and the frees.
//!
//! The static library this crate builds exports the five functions `include/kartta.h` declares:
//! `kartta_abi_version`, `kartta_open`, `kartta_call`, `kartta_string_free` and `kartta_free`. A method is a name and a
//! JSON object of params, and every answer is JSON, `{"ok": <answer>}` or `{"err": {"code": <code>, "message":
//! <message>}}`, so a binding translates JSON and decides nothing. The Go binding in `bindings/go` is one; a Rust
//! shell, such as a Node addon, builds on [`Kartta`] directly.
//!
//! A handle names a tree. Its `build` method walks the tree once, building the file index and the graph of every
//! language `kartta-languages` ships, or of those the params name; every query asks what the last finished build
//! made, and `refresh` brings that up to the tree in place. A build that asks to `watch` follows its tree instead: a
//! thread of its own refreshes the engine from each change the operating system reports, until the next build or the
//! handle's drop, and `stats` says whether it still does. Queries run concurrently; builds run one at a time, and a
//! query waits for a refresh under way, a watch's included. A build, a grep and a refresh each run under a cancel of
//! their own, which `cancel` trips by the `id` the call's caller gave it, or by the `binding_id` a binding gave it; an
//! id by one name never stops a call by the other, and none stops a watch.
//!
//! Answers are kartta's own: each carries the counts the engine holds of what a cap, a depth bound or a confidence
//! floor left out. `WIRE.md`, beside this crate's manifest, is the reference of every method, field, default, error code
//! and enumerated value.

#[cfg(not(panic = "unwind"))]
compile_error!(
    "kartta.h promises that no panic crosses the C ABI, which holds only under `panic = \"unwind\"`"
);

mod abi;
mod built;
mod error;
mod handle;
mod query;
mod wire;

pub use crate::error::Error;
pub use crate::handle::Kartta;

const ABI_VERSION: u32 = 1;
