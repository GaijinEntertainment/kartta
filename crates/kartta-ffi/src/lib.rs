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
//! made. Queries run concurrently; builds run one at a time, each under a cancel of its own that `cancel` trips.
//!
//! Answers are kartta's own: each carries the counts the engine holds of what a cap, a depth bound or a confidence
//! floor left out.

mod abi;
mod error;
mod handle;
mod wire;

pub use crate::error::{Error, ErrorCode};
pub use crate::handle::Kartta;

/// The version of the C functions' signatures and ownership rules, as `kartta_abi_version` returns it.
///
/// It changes only when a binding built against an older header would misuse the library; a new method or a new answer
/// field leaves it as it is.
pub const ABI_VERSION: u32 = 1;
