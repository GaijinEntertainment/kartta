# kartta-fs

Repository file index: one filesystem walk builds a snapshot, and every query answers from it.

`Index::build` reads a directory tree once, applying git's ignore rules on the way down, and keeps what survives. Every
question about *structure* is answered from that snapshot: a lookup, a directory listing and both path searches never
re-read the tree. Questions about a file's own bytes — its size, whether it is binary — are answered by reading it on
the first ask and keeping the answer. It takes an OS path and reads the real filesystem; there is no
virtual-filesystem abstraction to mock.

```rust
let outcome = Index::build(root, &BuildOptions::default())?;
report(&outcome.errors); // a partial index looks exactly like a complete one
let index = outcome.index;

let ranked = index.fuzzy("main", &FuzzyOptions::default());
assert_eq!(index.paths().get(ranked.items()[0].position), "src/main.rs");

let listed = index.list("src", &ListOptions::default())?; // what is in a place, depth-first
assert_eq!(index.size("src/main.rs"), Some(12)); // read now, kept
```

## Two rules on every surface

- **Paths are forward-slash and root-relative on every platform.** Windows is a first-class target, so no surface
  returns an OS-native separator where a path names an entry. Two surfaces are native instead, because both point at a
  place on disk rather than at an entry: `Index::root`, which an entry's path is joined onto to open it, and
  `BuildError::native_path`, which tells an operator where to go and look. One form for identity, the other for access.
- **A symlink is reported, never read through.** Its content is either already indexed under the target's own path or is
  not the repository's at all, so following one would either duplicate a file under a second path or read outside the
  tree. It is never descended and never stat'd through — a dangling link and a live one are indistinguishable to every
  query here, so the syscall that tells them apart would buy nothing. Indexed is therefore not the same as readable, by
  rule rather than by accident.
  The rule covers the tree as walked, not a writer changing it underneath: a directory replaced by a symlink before
  the walk lists it is descended through, and on Windows a file replaced by a symlink before a later read is read
  through.

## Structure is a snapshot; a file's bytes are not

The walk records what exists and what each entry is, so those answers are coherent across the whole tree and change only
when a walk runs again: a rebuild, or `Index::refresh` and `Index::refresh_paths`, which change them in place.
`Index::size` and `Index::is_binary` read the filesystem when first asked and keep what they read, because doing either
during the walk costs far more than the answers are worth to a caller who looks at a handful of files — on a 1.4M-entry
tree the walk-time stat alone was about half the build, and classifying every file runs to minutes. Two such answers may
therefore describe two different moments, and none is refreshed once taken.

A consumer that needs file content anyway — a parser, a content index — builds with `Index::build_with` and a
`ContentVisitor` instead. The build then reads each file the visitor wants once, while the walk is in its directory,
hands a text file's whole content to the visitor, and keeps the size and binary verdict that read learned, so the
consumer never opens the tree a second time after the walk. A build given `BuildOptions::content_cache` also lends the
visitor the shared store, keyed by each file's content through `Content::cached` and `Content::cache`, so what the
consumer computes per file is computed once across builds.

`Index::is_binary` settles a file by extension where the name names a format that is binary by construction, and by
whether a NUL appears in the first 64 KiB otherwise. The extension set only ever says "binary": listing a text format
wrongly would make it unfindable, where omitting a binary one costs a single read that answers correctly anyway. Two
built-in names are not binary by construction. `svg` is XML, listed because in practice it holds machine-emitted path
data, so an SVG reports binary without being read. `dag` is Dagor's binary format and also names the text workflow
files of the Condor job scheduler, so a tree that holds those states a set of its own.

`BuildOptions::binary_extensions` sets which extensions decide. The default is the built-in set; a caller adds its own
asset formats to it by union, or states a whole set of its own. An extension no file carries is not an error, because
most of the built-in set is absent from any one tree.

## Ignore rules

`.git` is never indexed and never descended: it holds the repository's bookkeeping, not the tree the repository
describes. A `.git` found deeper marks another repository's root, and the inherited rule stack resets there, exactly as
git's own rules stop at a repository boundary — its contents are still indexed, because this crate indexes a directory
tree rather than a git index.

`BuildOptions::pruned_names` names what else is never indexed and never descended: the clutter an editor or the
operating system leaves in a tree. The default is `.idea`, `.vscode`, `.fleet` and `.DS_Store`; a caller adds its own
names to that set by union, or states a `PrunedNames` of its own, which prunes nothing beyond `.git` when empty. An
entry so named is pruned at any depth and whatever its kind, before any ignore rule is
consulted, so no negation brings it back and `IgnoreSources::None` keeps it out too. While ignore rules apply,
a pruned `.gitignore` still applies its rules, as an ignored one does, so changing it refreshes its directory; a
refresh prunes the names the build was given, and the Linux watcher watches no directory they name. `.git` is not in
the set, because it marks where a repository's rules start rather than clutter a caller may want back. A misspelled
name needs no report: what it was meant to prune is in the index for anyone to see, unless the ignore rules hide it
too. A listing asked for a path through a pruned name, `.git` included, fails as `ListError::Pruned` and names the
name, so the caller learns why it found nothing rather than only that it did.

`BuildOptions::ignore_sources` selects which ignore files apply, and the default is the reproducible level: in-tree
`.gitignore` files only, which is the level at which the same tree indexes identically wherever it sits. Sources outside
the tree — `$GIT_DIR/info/exclude`, `core.excludesFile`, the XDG fallback — are machine-local, so honouring one gives
that up: the same tree then indexes differently on a laptop and in CI. That is git's behaviour and sometimes what a
caller wants, but never something to switch on by accident, which is why it is an enumerated field rather than a flag.

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
unstartable thread pool are fatal. `Index::build_with` takes a cancel token too, and a cancellation is returned rather
than survived, as `BuildError::Stopped`, whose `Stopped` hands back the index of whichever entries the threads reached
and counts those it turned away. Each surviving kind costs something different, and the difference is the point:

- An unreadable directory costs its whole subtree, and an entry whose type will not resolve costs that entry.
- An unreadable `.gitignore` costs its rules, so the index *holds* what that file would have hidden.
- A name that is not valid UTF-8 costs that entry, and a directory takes its subtree with it.
- A file a `ContentVisitor` wanted and the build could not read costs the visitor that file's content; the entry stays.

A build with neither a visitor nor `BuildOptions::content_index` never reads a file's size, so nothing here reports a
failure to. A size or a binary verdict that cannot be read answers `None` at the point of the ask instead, where the
caller can act on it.

The report is ordered by path, so two builds of one tree report identically.

Checking it is not optional. An index missing a subtree, or holding the files an unread `.gitignore` would have hidden,
is indistinguishable from a correct one — and the second case is the dangerous one, because a missing subtree is a
visible gap where files git would hide look exactly like files git would keep.

## Path search

`fuzzy` ranks paths against a query, delegating to `kartta-fuzz`: a hit inside a filename beats a hit inside a
directory name, so `main` finds `src/main.rs` before `main/lib.rs`. Directories are not ranked — a path search answers
"which file did you mean", and a matching directory would otherwise arrive alongside every file beneath it that matches
for the same reason. Matching ignores case, which is this crate's choice and not git's: `core.ignoreCase` describes what
the filesystem does with case, a different question from what a search should do with it.

A ranking answers how many paths matched beside the ones it returns, so ten paths read as ten of however many matched.
A limit keeps the best of every match it scores, so the count is a tally of that work. An empty query is not counted:
it matches every path in index order, so under a limit it stops there without reading the rest, and without one it
returns every match. A glob counts its matches as well: under a limit it reads on past the last path it returns and
matches the filter against every path left, on the index's threads.

`glob` and `fuzzy` take an exclusion beside the `Filter` that selects, as a listing and a content search do: a matched
directory is removed with everything beneath it before a limit sees it, so a capped answer stays exact. A ranking skips
what lies beneath an excluded directory without matching or scoring any of it; a glob matches its filter against each
path there once, to count what the exclusion removed.

The index keeps its paths front-coded, so results name entries by position and `Index::paths` reads each path back —
cheaply in index order, from the nearest full path otherwise. A ranking carries this crate's own `FuzzyMatch` rather
than the scorer's result type, so replacing the ranking engine stays a non-breaking change.

Import resolution is deliberately out of scope: mapping a specifier to a file is language semantics. What lives here are
the mechanical primitives a resolver queries.

## Content search

`grep` searches the content of regular files for a `Patterns` union, line by line, through ripgrep's own matcher and
searcher, so a pattern written for `rg` means the same here. Files are streamed rather than read whole, so no size bound
is needed and none is applied unless asked for. The binary rule is `is_binary`'s, applied before a byte is searched,
and the size and verdict a search learns are kept for the next one.

```rust
let patterns = Patterns::new([r"\bG_ASSERT\b"], Case::Sensitive)?;
let found = index.grep(&patterns, &GrepOptions::default().context(2).limit(100));
if found.truncated() || found.cancelled() || !found.failures().is_empty() {
    // not the whole answer: the limit cut it short, a cancel stopped it, or a file would not read
}
```

The answer is in index order, so it is the same on every run that is not cancelled, and it accounts for every candidate
— searched, binary, oversized, unreadable, ruled out unread by the content index, or left unsearched once a match past
the limit proved the cut or a cancellation stopped the search. A limited search reads on past a filled limit only until
that one more match, so `truncated` is exact; finding none, it reads every searchable candidate, as a search without a
limit does. `count_all` reads every searchable candidate and counts the matches past the limit.

A build given `BuildOptions::content_index` also records each text file's trigrams as the walk reads it, and a search
then rules out, before opening it, every indexed file missing a trigram its patterns need; a file the index holds no
trigrams for, such as one the build could not read, stays a candidate. Builds of nearly identical trees can share that
work through `BuildOptions::content_cache`, a `kartta-cache` directory keyed by each file's content: a file whose
trigrams another build already computed is hashed and looked up rather than scanned. A file of 64 KiB or more that no
`ContentVisitor` wants is read once to hash it and, when the store lacks it, again to scan it. Each build ends with a
merge that expires the entries older than `BuildOptions::content_cache_ttl`, a week when left at zero, and rewrites a
pack without them once they hold half of its payload bytes.

`examples/probe.rs` indexes a real tree and reports what survived, how long the build took, and what a path search costs
— a synthetic fixture cannot tell you magnitude, because a repository's `.gitignore` files are what decide how much of
it is indexed at all.

`examples/grep.rs` does the same for a content search, timing it twice and printing where every candidate went.

Status: the walk, the ignore rules, the snapshot, path lookup, the directory listing, sizes, binary classification,
both path searches — glob and fuzzy — each narrowed by a `Filter`, content search and the trigram index that narrows
it, refreshes of the whole tree or of named paths, change notifications behind the `watch` feature, and trigrams shared
through a content cache. Not yet: the ignore sources outside the tree.
