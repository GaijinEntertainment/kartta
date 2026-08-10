# kartta-fs

Repository file index with glob, fuzzy and regex queries over a single filesystem walk.

One walk builds one snapshot; the path queries, the content query and the directory listing all answer from it rather
than re-stating the tree. Gitignore handling is delegated to [`ignore`](https://crates.io/crates/ignore), fuzzy ranking
to `kartta-fuzz`, and regex content matching to ripgrep's [`grep-regex`](https://crates.io/crates/grep-regex) and
[`grep-searcher`](https://crates.io/crates/grep-searcher) — same dialects and diagnostics as those tools.

Two rules hold on every surface:

- Paths are forward-slash and repository-relative on every platform.
- A symlink is reported, never read through. Its content is either already indexed under the target's own path or is
  outside the repository entirely.

Import resolution is deliberately out of scope: mapping a specifier to a file is language semantics. This crate
provides the mechanical primitives a resolver queries.

Status: the API is being ported into this repository and is not published yet.
