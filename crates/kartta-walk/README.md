# kartta-walk

Parallel filesystem tree walk with caller-controlled descent and cancellation.

A walk returns the tree it discovered. A directory's entries arrive together, name-sorted, and its subtrees are walked
in parallel — so parent links never need reconstructing and there is no global sort to pay for afterwards.

## Scope

The crate walks and reports structure; it does not model what it finds. Each node carries whatever the `Policy`
returned, so the entry model — kinds, sizes, symlink targets — belongs to the consumer, as do a path lookup, any flat
projection of the tree, ignore-file semantics, and every query.

`walk` returns a `WalkOutcome`: a `Tree` plus a report about the run (what could not be read, whether it was cancelled).
The report is checked and dropped; the tree is the only part worth keeping. A `Tree` carries no diagnostics precisely so
it cannot be mistaken for a record of how it was obtained.

It exists to feed a repository file index. A tree is what this returns because a tree is what a traversal discovers, and
reconstructing one costs a second owned copy of every path. What the consumer keeps is its own affair: the index this
crate feeds flattens the tree into a depth-first list in one pass and answers even its directory listings from that.
Building that list here would put the consumer's data structure in the walker.

`examples/survey.rs` reports what a real tree holds and how long walking it takes at each thread count. There is no
benchmark: a fixture small enough to generate for one hides the costs that only appear on a tree of millions of
entries.

## The policy

A `Policy` decides everything the walk records. `enter` derives the state one directory's entries inherit from their
parent — where an accumulating rule set such as a `.gitignore` stack lives — and `visit` classifies each entry: record
it and descend, record it but treat it as opaque, or drop it. The walk itself knows nothing about ignore files, hidden
entries, or name conventions.

Two properties it commits to:

- **Deterministic.** Entries are sorted per directory before the policy sees them and subtree results collect in that
  same order, so an unchanged tree walks to the same tree at any thread count. The values in it are the policy's: a
  policy whose answer for an entry depends only on that entry and its directory's state walks to identical output, and
  one that answers by call order — numbering entries as it meets them, say — does not, because directories are walked
  in parallel and a large directory's entries are classified in parallel too.
- **Symlinks are never followed.** A symlink is reported as one and never descended, so the walk stays inside the tree
  it was given and cycle detection is unnecessary.

A `CancelToken` ends a walk early, returning the partial tree and saying so through `cancelled`, with `unvisited`
counting the listed entries the cancel turned away; stopping is a truncated answer, not an error. Likewise an unreadable
subdirectory is collected into the result's error list rather than failing the walk.

There is deliberately no cap on entries recorded. Truncating a walk returns an arbitrary subset of the tree — which
entries survive depends on scheduling — and a consumer cannot tell that from a complete tree without asking, so a
resource guard becomes a source of quietly wrong answers. A bound worth having would fail the walk instead. Cancellation
escapes the objection only because the caller who cancelled knows the tree is cut, and by how much.

The walk runs on the rayon pool it is called from, so a caller that does not own the machine — a container with a CPU
quota, a process under a thread limit — bounds its parallelism by installing a pool of the size it may use around the
call. Only an unreadable root and a subtree that may lead outside it are fatal.

`listed_stats` lists each directory with its regular files' lengths and inode change times where the platform's listing
carries them — macOS, through `getattrlistbulk` — so a policy that compares files with what it saw before pays no stat
per file. Such a listing costs more than one without them, so a walk lists them only when asked.
