# kartta-walk

Parallel filesystem tree walk with caller-controlled descent and cancellation.

A walk returns the tree it discovered. A directory's entries arrive together, name-sorted, and its subtrees are walked in
parallel — so parent links never need reconstructing and there is no global sort to pay for afterwards.

## Scope

The crate walks and reports structure; it does not model what it finds. Each node carries whatever the `Policy` returned,
so the entry model — kinds, sizes, symlink targets — belongs to the consumer, as do a path lookup, any flat projection of
the tree, ignore-file semantics, and every query.

`walk` returns a `WalkOutcome`: a `Tree` plus a report about the run (what could not be read, whether it was cancelled).
The report is checked and dropped; `into_tree` hands over the structure, which is the only part worth keeping. A `Tree`
carries no diagnostics precisely so it cannot be mistaken for a record of how it was obtained.

It exists to feed a filesystem index. An index answers a directory listing from a tree, so a tree is what this returns.
An index that also needs a flat path list — for fuzzy path search — derives one from the tree in a single pass, on its own
side of the boundary; building it here would put the consumer's data structure in the walker.

`examples/survey.rs` reports what a real tree holds and how long walking it takes at each thread count;
`benches/walk.rs` tracks regressions against a small deterministic fixture.

## The policy

A `Policy` decides everything the walk records. `enter` derives the state one directory's entries inherit from their
parent — where an accumulating rule set such as a `.gitignore` stack lives — and `visit` classifies each entry: keep it
and descend, keep it but treat it as opaque, or drop it. The walk itself knows nothing about ignore files, hidden
entries, or name conventions.

Two properties it commits to:

- **Deterministic.** Entries are sorted per directory before the policy sees them and subtree results collect in that
  same order, so an unchanged tree walks to identical output at any thread count.
- **Symlinks are never followed.** A link is reported as one and never descended, so the walk stays inside the tree it
  was given and cycle detection is unnecessary.

A `CancelToken` ends a walk early, returning the partial tree with `cancelled` set; stopping is a truncated answer, not
an error. Likewise an unreadable subdirectory is collected into the result's error list rather than failing the walk.

There is deliberately no cap on entries recorded. Truncating a walk returns an arbitrary subset of the tree — which
entries survive depends on scheduling — and a consumer cannot tell that from a complete tree without asking, so a
resource guard becomes a source of quietly wrong answers. A bound worth having would fail the walk instead. Cancellation
escapes the objection only because the caller who cancelled is the one discarding the result.

`threads` bounds the parallelism itself, for a caller that does not own the machine — a container with a CPU quota, a
process under a thread limit. The count is exact, so `Some(1)` walks sequentially, and a pool that cannot be started is
a returned error rather than a silent fall back to some other parallelism. Only that and an unreadable root are fatal.

Status: the API is being ported into this repository and is not published yet.
