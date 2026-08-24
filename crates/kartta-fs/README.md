# kartta-fs

Repository file index: one filesystem walk builds a snapshot, and every query answers from it.

`Index::build` reads a directory tree once, applying git's ignore rules on the way down, and keeps what survives.
Nothing after that touches the filesystem — a lookup, a path query and, in time, a content search all read the same
snapshot rather than re-stating the tree. It takes an OS path and reads the real filesystem; there is no
virtual-filesystem abstraction to mock.

```rust
let outcome = Index::build(root, &Config::default())?;
report(outcome.errors()); // a partial index looks exactly like a complete one
let index = outcome.into_index();

let ranked = index.fuzzy_paths("main", None);
assert_eq!(ranked[0].path, "src/main.rs");
```

## Two rules on every surface

- **Paths are forward-slash and root-relative on every platform.** Windows is a first-class target, so no surface
  returns an OS-native separator. `Index::root` is the single exception, because opening a file needs the platform's own
  spelling: one form for identity, the other for access.
- **A symlink is reported, never read through.** Its content is either already indexed under the target's own path or is
  not the repository's at all, so following one would either duplicate a file under a second path or read outside the
  tree. It is never descended and never stat'd through — a dangling link and a live one are indistinguishable to every
  query here, so the syscall that tells them apart would buy nothing. Listed is therefore not the same as readable, by
  rule rather than by accident.

## Ignore rules

`.git` is never indexed and never descended: it holds the repository's bookkeeping, not the tree the repository
describes. A `.git` found deeper marks another repository's root, and the inherited rule stack resets there, exactly as
git's own rules stop at a repository boundary — its contents are still indexed, because this crate indexes a directory
tree rather than a git index.

`Config::ignore` selects which ignore files apply, and the default is the reproducible level: in-tree `.gitignore` files
only, which is the level at which the same tree indexes identically wherever it sits. Sources outside the tree —
`$GIT_DIR/info/exclude`, `core.excludesFile`, the XDG fallback — are machine-local, so honouring one gives that up: the
same tree then indexes differently on a laptop and in CI. That is git's behaviour and sometimes what a caller wants, but
never something to switch on by accident, which is why it is an enumerated field rather than a flag.

The guarantee is over the tree on disk, never over a commit. A build reads the working tree and not git's index, so two
checkouts of one commit index differently whenever their untracked, unignored content differs.

An ignore file that is a symlink pointing out of the tree is refused and reported rather than followed — otherwise a
repository could decide its own index from bytes that do not travel with it, which is the machine-dependence the
in-tree level exists to keep out. A symlink to a target inside the tree is followed, as git follows it.

Matching is delegated to [`gix-ignore`](https://crates.io/crates/gix-ignore), which runs git's own `wildmatch` over
base-relative pattern lists. Git's dialect is a compatibility surface: a divergence is a silently wrong index rather
than a crash, so it is not reimplemented.

What is modelled is git's **ignore rules**, not git's index. A file git already tracks stays tracked once a rule would
hide it, and such a file is not indexed here — `git check-ignore --no-index` is the command that agrees with this
crate, and `git ls-files` is the one that does not.

## A build reports what it could not read

`Index::build` returns a `BuildOutcome`: the index plus the failures the build survived. Only an unlistable root and an
unstartable thread pool are fatal. Each surviving kind costs something different, and the difference is the point:

- An unreadable directory costs its whole subtree, and an entry whose type will not resolve costs that entry.
- An unreadable `.gitignore` costs its rules, so the index *holds* what that file would have hidden.
- A name that is not valid UTF-8 costs that entry, and a directory takes its subtree with it.
- A failed stat costs only a size — the entry is still indexed, with `size()` answering `None`.

The report is ordered by path, so two builds of one tree report identically, and `into_index` hands over the snapshot.

Checking it is not optional. An index missing a subtree, or holding the files an unread `.gitignore` would have hidden,
is indistinguishable from a correct one — and the second case is the dangerous one, because a missing subtree is a
visible gap where files git would hide look exactly like files git would keep.

## Queries

`fuzzy_paths` ranks paths against a query, delegating to `kartta-fuzz`: a hit inside a filename beats a hit inside a
directory name, so `main` finds `src/main.rs` before `main/lib.rs`. Directories are not ranked — a path query answers
"which file did you mean", and a matching directory would otherwise arrive alongside every file beneath it that matches
for the same reason. Matching ignores case, which is this crate's choice and not git's: `core.ignoreCase` describes what
the filesystem does with case, a different question from what a search should do with it.

Results borrow from the index and carry this crate's own `PathMatch` rather than the scorer's result type, so replacing
the ranking engine stays a non-breaking change.

Import resolution is deliberately out of scope: mapping a specifier to a file is language semantics. What lives here are
the mechanical primitives a resolver queries.

`examples/probe.rs` indexes a real tree and reports what survived, how long the build took, and what a query costs — a
synthetic fixture cannot tell you magnitude, because a repository's `.gitignore` files are what decide how much of it is
indexed at all.

Status: the walk, the ignore rules, the snapshot, path lookup and the fuzzy path query. Not yet: glob and regex queries,
directory listing, per-query filters, incremental re-walk, and the ignore sources outside the tree.
