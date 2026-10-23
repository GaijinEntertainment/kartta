# ADR Index

## Accepted

- [1.1 Own the filesystem walk](./adr/1.1-own-the-filesystem-walk.md)
  - Governs: `crates/kartta-walk`, `kartta_walk::walk`, `kartta_walk::Policy`, `kartta_walk::Tree`
  - Revisit if: the walk falls behind on a real tree, a maintained crate returns grouped results with inherited state,
    or a re-walk needs what the seed state cannot express
- [1.2 Own the path scorer](./adr/1.2-own-the-path-scorer.md)
  - Governs: `crates/kartta-fuzz`, `kartta_fuzz::Ranking`
  - Revisit if: an interactive consumer appears and tens of milliseconds bind, or either matcher grows a per-position
    or per-segment scoring hook
- [1.3 Make the ignore source set explicit](./adr/1.3-make-the-ignore-source-set-explicit.md)
  - Governs: `crates/kartta-fs/src/build.rs`, `kartta_fs::IgnoreSources`, `kartta_fs::BuildOptions::ignore_sources`
  - Revisit if: a consumer needs full git fidelity by default
- [1.4 Take the query glob dialect from globset](./adr/1.4-take-the-query-glob-dialect-from-globset.md)
  - Governs: `crates/kartta-fs/src/filter.rs` (`Filter`, `FilterError`)
  - Revisit if: `gix-glob` grows alternation and pattern validation, or compile time becomes the driver
- [1.5 Announce a cap unless the order already explains it](./adr/1.5-announce-a-cap-unless-the-order-explains-it.md)
  - Governs: `crates/kartta-fs/src/glob.rs` (`Capped::truncated`), `crates/kartta-fs/src/list.rs`
    (`Listing::truncated`), `crates/kartta-fs/src/fuzzy.rs` (`FuzzyMatches`), `Index::entries`
  - Revisit if: a caller needs the count of what a cap cut on an index-order surface
- [1.6 Read a file's own facts on demand, not during the walk](./adr/1.6-read-a-file-s-own-facts-on-demand.md)
  - Governs: `crates/kartta-fs/src/inspection.rs`, `Index::size`, `Index::is_binary`, `Listed::size`,
    `Listed::is_binary`
  - Revisit if: a caller needs file facts coherent with the walk for files a build did not read
- [1.7 Let a caller compose the binary extension set](./adr/1.7-let-a-caller-compose-the-binary-extension-set.md)
  - Governs: `crates/kartta-fs/src/binary.rs` (`BinaryExtensions`, `BinaryExtensionsError`),
    `BuildOptions::binary_extensions`
  - Revisit if: a caller needs a classification rule richer than a name's last extension
- [1.8 Count what a listing leaves out](./adr/1.8-count-what-a-listing-leaves-out.md)
  - Governs: `crates/kartta-fs/src/list.rs` (`ListOptions`, `Listing`, `Listed::unlisted`, `Index::list`)
  - Revisit if: counting stops being free, as in an index layout without contiguous subtrees
- [1.9 Front-code the stored paths](./adr/1.9-front-code-the-stored-paths.md)
  - Governs: `crates/kartta-fs/src/index.rs`, `crates/kartta-fs/src/list.rs`, `kartta_fs::Entry`,
    `kartta_fuzz::Ranking`
  - Revisit if: a call nears 500 ms on a larger tree
- [1.10 Search content through ripgrep's engine](./adr/1.10-search-content-through-ripgrep-s-engine.md)
  - Governs: `crates/kartta-fs/src/scan.rs`, `crates/kartta-fs/src/patterns.rs`, `crates/kartta-fs/src/open.rs`,
    `crates/kartta-fs/src/grep.rs` (`Planner::admit`), `Index::grep`, `GrepOptions`
  - Revisit if: the dependency's weight matters more than the code it saves, or agents need to search UTF-16 sources
- [1.11 Say what a limited content search did not
  cover](./adr/1.11-say-what-a-limited-content-search-did-not-cover.md)
  - Governs: `crates/kartta-fs/src/grep.rs` (`Planner`, `Fold`), `crates/kartta-fs/src/matches.rs`, `Matches`,
    `GrepOptions::limit`, `GrepOptions::count_all`
  - Revisit if: a content index can count matches without reading files, so `count_all` could be the default
- [1.12 Read each wanted file once, during the walk](./adr/1.12-read-each-wanted-file-once-during-the-walk.md)
  - Governs: `crates/kartta-fs/src/{policy,content}.rs`, `Index::build_with`, `ContentVisitor`, `ContentId`,
    `Index::content_position`, `crates/kartta-walk/src/walk.rs` (`WalkOutcome::cancelled`)
  - Revisit if: a consumer needs content in pieces rather than whole
- [1.13 Let the caller bound the index's threads](./adr/1.13-let-the-caller-bound-the-index-s-threads.md)
  - Governs: `kartta_fs::BuildOptions::threads`, `kartta_fs::Index::threads`, `kartta_fs::Index::on_threads`,
    `crates/kartta-walk/src/walk.rs`
  - Revisit if: searches and builds need different bounds
- [1.14 Narrow content search with a trigram index built from the walk's
  reads](./adr/1.14-narrow-content-search-with-a-trigram-index.md)
  - Governs: `crates/kartta-fs/src/content_index.rs`, `crates/kartta-fs/src/requirement.rs`,
    `crates/kartta-fs/src/trigram.rs`, `crates/kartta-fs/src/postings.rs`, `BuildOptions::content_index`,
    `Matches::ruled_out`
  - Revisit if: query logs show false candidates dominating a search, or the directory becomes a noticeable part of the
    index
- [1.15 Refresh an index in place from a second walk](./adr/1.15-refresh-an-index-in-place-from-a-second-walk.md)
  - Governs: `crates/kartta-fs/src/index.rs` (`Index::refresh`), `crates/kartta-fs/src/index/refresh.rs`
  - Revisit if: a consumer refreshes often enough to near the last id, or retired ids leave the id map far past the
    files that remain
- [1.16 Refresh the parts of a tree a caller names](./adr/1.16-refresh-the-parts-of-a-tree-a-caller-names.md)
  - Governs: `crates/kartta-fs/src/index/{scoped,splice}.rs` (`Index::refresh_paths`, `Index::parts_of`)
  - Revisit if: the copy of the per-entry arrays sets a follower's peak, or becomes most of a call
- [1.17 Follow a tree through its change notifications](./adr/1.17-follow-a-tree-through-its-change-notifications.md)
  - Governs: `crates/kartta-fs/src/watch.rs`, `crates/kartta-fs/src/watch/`, `kartta_fs::Watcher`,
    `Index::{build,refresh,refresh_paths}_watched`, `crates/kartta/src/follow.rs`
  - Revisit if: a consumer that cannot run a loop of its own needs to follow a tree
- [1.18 Return the memory a build freed to the system](./adr/1.18-return-the-memory-a-build-freed-to-the-system.md)
  - Governs: `crates/kartta-fs/src/index.rs`, `crates/kartta/src/engine.rs` (`release_freed_memory`)
  - Revisit if: a consumer's resident memory grows across scoped refreshes
- [1.19 Prune editor and OS clutter by name during the
  walk](./adr/1.19-prune-editor-and-os-clutter-by-name-during-the-walk.md)
  - Governs: `crates/kartta-fs/src/pruned.rs`, `BuildOptions::pruned_names`, `crates/kartta-fs/src/policy.rs`
    (`pruned`, `Indexing::visit`), `Index::refresh_paths`
  - Revisit if: a consumer needs what a pruned directory holds, a name outside the default proves common, or a caller
    needs a path-shaped prune
- [1.20 Watch on Linux each directory the index keeps, from the walk that keeps
  it](./adr/1.20-watch-on-linux-each-directory-the-index-keeps-from-the-walk-that-keeps-it.md)
  - Governs: `crates/kartta-fs/src/watch/inotify.rs`, `Index::refresh_paths_watched`, `WatchError::Limit`
  - Revisit if: followed trees outrun `fs.inotify.max_user_watches`, or the walk's queue on watch calls costs a build
    more than a whole refresh
- [1.21 Bound a Linux drain by the most events its queue can
  hold](./adr/1.21-bound-a-linux-drain-by-the-most-events-its-queue-can-hold.md)
  - Governs: `crates/kartta-fs/src/watch/inotify.rs` (`Backend::poll`, `Backend::queued`), `Watcher::drain`
  - Revisit if: the kernel offers a count of an inotify queue that takes no lock writers share
- [1.22 Make the inotify watch map anew when it is short of
  room](./adr/1.22-make-the-inotify-watch-map-anew-when-it-is-short-of-room.md)
  - Governs: `crates/kartta-fs/src/watch/inotify.rs` (`Walked::into_dirs`, `Backend::release`)
  - Revisit if: the standard library's hash map stops leaving tombstones that keep room
- [1.23 Ship kartta-fs on the newest notify release
  candidate](./adr/1.23-ship-kartta-fs-on-the-newest-notify-release-candidate.md)
  - Governs: `crates/kartta-fs/Cargo.toml` (`notify`), `crates/kartta-fs/src/watch/notified.rs`
  - Revisit if: notify publishes 9.0.0, or a candidate after rc.6
- [1.24 Watch the macOS data volume through its firmlinks in one
  stream](./adr/1.24-watch-the-macos-data-volume-through-its-firmlinks-in-one-stream.md)
  - Governs: `crates/kartta-fs/src/watch.rs` (`event_prefixes`, `through_firmlinks`, `relative`),
    `crates/kartta-fs/src/watch/notified.rs` (`Backend::start`)
  - Revisit if: notify adds a `watch` call's paths to a running stream, or macOS names data-volume events by that path
- [1.25 Queue macOS and Windows notifications in a vector that
  grows](./adr/1.25-queue-macos-and-windows-notifications-in-a-vector-that-grows.md)
  - Governs: `crates/kartta-fs/src/watch/notified.rs` (`QUEUED`, `Queue`, `Backend::poll`)
  - Revisit if: followed trees overflow the queue during a build as a matter of course
- [1.26 Hold a path a notification in a batch, each at its
  length](./adr/1.26-hold-a-path-a-notification-in-a-batch-each-at-its-length.md)
  - Governs: `crates/kartta-fs/src/watch.rs` (`Found`, `Watcher::changes`), `crates/kartta-fs/src/watch/inotify.rs`
  - Revisit if: a consumer raises `fs.inotify.max_queued_events` far past a million
- [1.40 Carry an unchanged file over by its size and change
  time](./adr/1.40-carry-an-unchanged-file-over-by-its-size-and-change-time.md)
  - Governs: `crates/kartta-fs/src/stamp.rs`, `BuildOptions::stamps`, `kartta_walk::WalkOptions::listed_stats`
  - Revisit if: a consumer refreshes a part small enough for the vnode cache so often that an lstat costs less than the
    listing
- [1.41 Index what a refresh reads in a segment of its
  own](./adr/1.41-index-what-a-refresh-reads-in-a-segment-of-its-own.md)
  - Governs: `crates/kartta-fs/src/content_index.rs`, `crates/kartta-fs/src/content_index/{compact,build}.rs`
  - Revisit if: retired ids leave a tree's merged lists sized far past the files that remain
- [1.42 Refresh a path under the name the disk holds](./adr/1.42-refresh-a-path-under-the-name-the-disk-holds.md)
  - Governs: `crates/kartta-fs/src/spelling.rs`, `crates/kartta-fs/src/index/scoped.rs`
  - Revisit if: a consumer on Linux follows a tree on a volume that folds case
- [1.43 Read the rules above a refreshed part again on each
  call](./adr/1.43-read-the-rules-above-a-refreshed-part-again-on-each-call.md)
  - Governs: `crates/kartta-fs/src/index/scoped.rs` (`Rewalk::rules_above`), `crates/kartta-fs/src/policy.rs`
  - Revisit if: the parts' own stats leave the calling thread, so the rules become the larger share of a call over a
    network mount
- [1.44 Answer where a refresh walked and where its kept entries
  went](./adr/1.44-answer-where-a-refresh-walked-and-where-its-kept-entries-went.md)
  - Governs: `RefreshOutcome`, `crates/kartta-fs/src/index/changed.rs`
  - Revisit if: a consumer needs to know which entries inside a walked part kept their position
- [1.45 Read a named refresh's files on the index's
  threads](./adr/1.45-read-a-named-refresh-s-files-on-the-index-s-threads.md)
  - Governs: `crates/kartta-fs/src/index/scoped.rs` (`Rewalk::found`, `Rewalk::read_on_threads`)
  - Revisit if: the stats and rules the calling thread keeps become a large share of a call
- [1.50 Match gitignore rules with gix-ignore](./adr/1.50-match-gitignore-rules-with-gix-ignore.md)
  - Governs: `crates/kartta-fs/src/rules.rs`
  - Revisit if: `gix-ignore` stops being maintained or diverges from git's matching
- [1.51 Score paths with fixed weights](./adr/1.51-score-paths-with-fixed-weights.md)
  - Governs: `crates/kartta-fuzz/src/score.rs`, `crates/kartta-fuzz/src/class.rs`, `crates/kartta-fuzz/src/ranking.rs`
  - Revisit if: a caller needs another point in the scoring space
- [1.52 Name the ignore files an index applied](./adr/1.52-name-the-ignore-files-an-index-applied.md)
  - Governs: `crates/kartta-mcp/src/state.rs` (`Gitignore`), `crates/kartta-mcp/src/render.rs` (`index`)
  - Revisit if: kartta reads the `.gitignore` files above a root
- [1.53 Find git's machine-local ignore files through git
  itself](./adr/1.53-find-git-s-machine-local-ignore-files-through-git-itself.md)
  - Governs: `kartta_fs::IgnoreSources` (the levels above `InTree`, not implemented yet)
  - Revisit if: `gix-ignore` resolves `core.excludesFile` itself without the configuration dependency tree
- [1.54 Model git's ignore rules, not git's index](./adr/1.54-model-git-s-ignore-rules-not-git-s-index.md)
  - Governs: `crates/kartta-fs/src/policy.rs`, `crates/kartta-fs/src/rules.rs`
  - Revisit if: repositories whose rules were added after their files turn out common
- [1.55 Exclude from a path query before its limit](./adr/1.55-exclude-from-a-path-query-before-its-limit.md)
  - Governs: `crates/kartta-fs/src/filter.rs` (`Filter`), `crates/kartta-fs/src/glob.rs` (`GlobOptions`, `Capped`),
    `crates/kartta-fs/src/fuzzy.rs` (`FuzzyOptions`), `crates/kartta-fs/src/index.rs` (`Index::candidates`)
  - Revisit if: the one match per path beneath an excluded directory shows in a glob's profile on a large tree
- [1.56 Count a ranking's matches as it scores them](./adr/1.56-count-a-ranking-s-matches-as-it-scores-them.md)
  - Governs: `crates/kartta-fs/src/fuzzy.rs` (`FuzzyMatches::matched`), `crates/kartta-fuzz/src/ranking.rs`
    (`Ranking::matched`, `Ranking::merge`)
  - Revisit if: the ranking stops scoring every candidate the filter and the exclusion leave
- [1.60 Settle a file as binary by its extension, else by a NUL in its first 64
  KiB](./adr/1.60-settle-a-file-as-binary-by-its-extension-else-by-a-nul-in-its-first-64-kib.md)
  - Governs: `Index::is_binary`, `crates/kartta-fs/src/inspection.rs` (`PROBE`), `crates/kartta-fs/src/binary.rs`
    (`BUILTIN`), `crates/kartta-fs/src/scan.rs`
  - Revisit if: a tree shows that the extension set's shape is wrong, not only its contents
- [1.61 Classify a large directory's entries in
  parallel](./adr/1.61-classify-a-large-directory-s-entries-in-parallel.md)
  - Governs: `crates/kartta-walk/src/walk.rs` (`PARALLEL_CLASSIFY`, `CLASSIFY_RUN`, `Walker::classify`,
    `Walker::classify_in_parallel`)
  - Revisit if: the policy's `visit` stops doing I/O for every consumer
- [1.62 Name the pruned name a listing failed on, and count nothing
  pruned](./adr/1.62-name-the-pruned-name-a-listing-failed-on-and-count-nothing-pruned.md)
  - Governs: `ListError::Pruned`, `crates/kartta-fs/src/list.rs` (`Index::not_found`)
  - Revisit if: a consumer needs to know a directory held pruned entries without asking a path through one
- [1.65 Re-encode only the blocks a change lands in](./adr/1.65-re-encode-only-the-blocks-a-change-lands-in.md)
  - Governs: `crates/kartta-fs/src/index/splice.rs`, `crates/kartta-fs/src/index.rs` (`Block`, `block_of`)
  - Revisit if: the per-block search shows in the profile of reads by position
- [1.66 Keep a symlink's target in a list beside the walk's
  tree](./adr/1.66-keep-a-symlink-s-target-in-a-list-beside-the-walk-s-tree.md)
  - Governs: `crates/kartta-fs/src/policy.rs` (`Payload`, `TargetId`, `Report::targets`)
  - Revisit if: a node must carry a second id beside its kind
- [1.67 Hold each extraction at its exact size](./adr/1.67-hold-each-extraction-at-its-exact-size.md)
  - Governs: `crates/kartta/src/extracting.rs` (`Extracting::extract`, `encoding_buffers`)
  - Revisit if: the build stops holding every encoding until resolution
- [1.70 Search content in batches folded in index
  order](./adr/1.70-search-content-in-batches-folded-in-index-order.md)
  - Governs: `crates/kartta-fs/src/grep.rs` (`BATCH`, `MIN_FILES_PER_SCANNER`, `Fold`), `crates/kartta-fs/src/scan.rs`
    (`Scanner`), `Index::grep`
  - Revisit if: a limited search's read past the match that proves its cut shows in an agent's latency, or a
    scanner's buffers outgrow the 16-file bound
- [1.71 Keep the content index in memory, a scratch file or a
  mapping](./adr/1.71-keep-the-content-index-in-memory-a-scratch-file-or-a-mapping.md)
  - Governs: `crates/kartta-fs/src/content_index/storage.rs`, `ContentIndexStorage`, `BuildError::ContentIndex`
  - Revisit if: a platform offers a write path that keeps written pages out of the resident set and reports a full disk
    as an error
- [1.72 Build the content index from sorted runs merged in
  place](./adr/1.72-build-the-content-index-from-sorted-runs-merged-in-place.md)
  - Governs: `crates/kartta-fs/src/content_index/build.rs`, `crates/kartta-fs/src/trigram.rs` (`Trigrams`),
    `crates/kartta-fs/src/policy.rs` (the reading lock)
  - Revisit if: the id lock shows on a build's wall time
- [2.1 Share per-file data through content-addressed
  packs](./adr/2.1-share-per-file-data-through-content-addressed-packs.md) — initiative `02-shared-cache`, 2026-10-16,
  revised 2026-10-18
- [2.2 Take per-file data from the shared cache in the walk's
  read](./adr/2.2-take-per-file-data-from-the-cache-in-the-walk-s-read.md) — initiative `02-shared-cache`, 2026-10-16,
  revised 2026-10-18
- [2.3 Expire cache entries by the time each was
  published](./adr/2.3-expire-cache-entries-by-the-time-each-was-published.md) — initiative `02-shared-cache`,
  2026-10-18, revised 2026-10-19
- [3.1 Plug languages in through a trait](./adr/3.1-plug-languages-in-through-a-trait.md) — initiative `03-code-graph`,
  2026-10-16, revised 2026-10-22
- [3.2 Name a Go call's target only where the syntax
  spells it](./adr/3.2-name-a-go-call-s-target-only-where-the-syntax-spells-it.md) — initiative `03-code-graph`,
  2026-10-16
- [3.3 Repair what the C++ grammar cannot parse instead of vendoring
  it](./adr/3.3-repair-what-the-c-plus-plus-grammar-cannot-parse.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-23
- [3.4 Extract TypeScript and JavaScript as one
  family](./adr/3.4-extract-typescript-and-javascript-as-one-family.md) — initiative `03-code-graph`, 2026-10-16,
  revised 2026-10-23
- [3.5 Keep the graph a file at a time](./adr/3.5-keep-the-graph-a-file-at-a-time.md) — initiative `03-code-graph`,
  2026-10-16, revised 2026-10-17
- [3.6 Build the graph inside the index's walk](./adr/3.6-build-the-graph-inside-the-index-s-walk.md) — initiative
  `03-code-graph`, 2026-10-16, revised 2026-10-21
- [3.7 Resolve calls when a query asks](./adr/3.7-resolve-calls-when-a-query-asks.md)
  - Governs: `crates/kartta-graph/src/{resolve,scope,calls,candidates,confidence}.rs`, `kartta_lang::Conventions`
  - Revisit if: a lazy `callers` of one name exceeds a second, or agents repeat names often enough that a memo pays for
    its invalidation
- [3.8 Extract only the daslang a file opts into gen2, and find modules by file
  name](./adr/3.8-extract-only-the-daslang-a-file-opts-into-gen2.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-23
- [3.9 Resolve a Quirrel import through the nearest .sqconfig, and name a function value by its
  binding](./adr/3.9-resolve-a-quirrel-import-through-the-nearest-sqconfig.md) — initiative `03-code-graph`, 2026-10-16,
  revised 2026-10-23
- [3.10 Resolve a BLK include as the engine loads it, and read the mount table from the files that state it by
  name](./adr/3.10-resolve-a-blk-include-as-the-engine-loads-it.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-17
- [3.11 Name Rust items by their module chain and resolve modules by Cargo's
  layout](./adr/3.11-name-rust-items-by-their-module-chain-and-resolve-modules-by-cargo-s-layout.md) — initiative
  `03-code-graph`, 2026-10-16, revised 2026-10-23
- [3.12 Qualify Python by the module its path
  names](./adr/3.12-qualify-python-by-the-module-its-path-names.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-23
- [3.13 Read a shell script's commands as calls and its sources as
  includes](./adr/3.13-read-a-shell-script-s-commands-as-calls-and-its-sources-as-includes.md) — initiative
  `03-code-graph`, 2026-10-16, revised 2026-10-19
- [3.14 Fold a PHP include path and map a use through
  PSR-4](./adr/3.14-fold-a-php-include-path-and-map-a-use-through-psr-4.md) — initiative `03-code-graph`, 2026-10-16,
  revised 2026-10-23
- [3.15 Qualify a Ruby member by its whole namespace
  chain](./adr/3.15-qualify-a-ruby-member-by-its-whole-namespace-chain.md) — initiative `03-code-graph`, 2026-10-16,
  revised 2026-10-23
- [3.16 Load a Lua module only from a literal name](./adr/3.16-load-a-lua-module-only-from-a-literal-name.md) —
  initiative `03-code-graph`, 2026-10-16, revised 2026-10-19
- [3.17 Resolve a Puppet class through the autoloader's
  layout](./adr/3.17-resolve-a-puppet-class-through-the-autoloader-s-layout.md) — initiative `03-code-graph`, 2026-10-16
- [3.18 Extract DSHL as one namespace that its includes
  join](./adr/3.18-extract-dshl-as-one-namespace-its-includes-join.md) — initiative `03-code-graph`, 2026-10-16
- [3.19 Find the files that include a file through an index of
  includers](./adr/3.19-find-the-files-that-include-a-file-through-an-index-of-includers.md) — initiative
  `03-code-graph`, 2026-10-17
- [3.20 Resolve implementors from the parents written when a query
  asks](./adr/3.20-resolve-implementors-from-the-parents-written-when-a-query-asks.md) — initiative `03-code-graph`,
  2026-10-17, revised 2026-10-23
- [3.21 Search definitions by the words of their names and
  paths](./adr/3.21-search-definitions-by-the-words-of-their-names-and-paths.md) — initiative `03-code-graph`,
  2026-10-17, revised 2026-10-21
- [3.22 Analyze what a change reaches with the walk the call queries
  take](./adr/3.22-analyze-what-a-change-reaches-with-the-walk-the-call-queries-take.md) — initiative `03-code-graph`,
  2026-10-17, revised 2026-10-23
- [3.23 Judge code dead only where the graph sees it
  used](./adr/3.23-judge-code-dead-only-where-the-graph-sees-it-used.md) — initiative `03-code-graph`, 2026-10-17,
  revised 2026-10-23
- [3.24 Count what a graph holds and how its calls bind when
  asked](./adr/3.24-count-what-a-graph-holds-and-how-its-calls-bind-when-asked.md) — initiative `03-code-graph`,
  2026-10-17
- [3.25 Read a definition's text and doc comment from its file when
  asked](./adr/3.25-read-a-definition-s-text-and-doc-comment-from-its-file-when-asked.md) — initiative
  `03-code-graph`, 2026-10-17, revised 2026-10-19
- [3.29 Name a Go receiver by the type its declarations
  spell](./adr/3.29-name-a-go-receiver-by-the-type-its-declarations-spell.md) — initiative `03-code-graph`, 2026-10-17,
  revised 2026-10-23
- [3.30 Look a repository path up without ASCII case through a table built when first
  asked](./adr/3.30-look-a-repository-path-up-without-case-through-a-table-built-when-first-asked.md) — initiative
  `03-code-graph`, 2026-10-17
- [3.31 Parse a C or C++ table without its numbers, and hold one tree at a
  time](./adr/3.31-parse-a-c-or-c-plus-plus-table-without-its-numbers.md) — initiative `03-code-graph`, 2026-10-18,
  revised 2026-10-22
- [3.32 Hand an engine every shipped language from one crate, and select them by
  name](./adr/3.32-hand-an-engine-every-shipped-language-from-one-crate.md) — initiative `03-code-graph`, 2026-10-18,
  revised 2026-10-22
- [3.33 Follow a tree through the questions each file's resolution
  asked](./adr/3.33-follow-a-tree-through-the-questions-each-file-s-resolution-asked.md) — initiative `03-code-graph`,
  2026-10-18, revised 2026-10-22
- [3.34 Record each read and write of a field or a value, and bind it when a query
  asks](./adr/3.34-record-each-read-and-write-of-a-field-or-a-value.md) — initiative `03-code-graph`, 2026-10-18,
  revised 2026-10-23
- [3.35 Pick a shared name's symbol by its path, and list the
  others](./adr/3.35-pick-a-shared-name-s-symbol-by-its-path-and-list-the-others.md) — initiative `03-code-graph`,
  2026-10-19
- [3.36 Keep a graph's files in path order through labelled
  blocks](./adr/3.36-keep-a-graph-s-files-in-path-order-through-labelled-blocks.md)
  - Governs: `crates/kartta-graph/src/order.rs`, `crates/kartta-graph/src/graph.rs` (`tree_order`, `path_rank`)
  - Revisit if: a followed tree inserts at one place in runs long enough that full relabels show in a refresh's profile
- [3.37 Count what a call walk leaves out](./adr/3.37-count-what-a-call-walk-leaves-out.md)
  - Governs: `crates/kartta-graph/src/calls.rs`, `Graph::callers`, `Graph::callees`, `Graph::references`,
    `Graph::call_path`, `Walk::self_call`
  - Revisit if: a count takes a second pass over the walk or a resolution the answer does not need
- [3.38 List a call under the last part of its callee's own
  name](./adr/3.38-list-a-call-under-the-last-part-of-its-callee-s-own-name.md)
  - Governs: `crates/kartta-graph/src/graph.rs` (`qualified_tail`, `listing_key`, `intern_listing_key`)
  - Revisit if: a language's callable names hold a separator inside them, not only at their end
- [3.39 Bind a member call to a free function only where a value can hold
  one](./adr/3.39-bind-a-member-call-to-a-free-function-only-where-a-value-can-hold-one.md)
  - Governs: `kartta_lang::MemberCalls`, `crates/kartta-graph/src/scope.rs`, the Go and Rust `Conventions`
  - Revisit if: the C++ extraction reads every class head, so no member reaches the graph as a free function
- [3.40 Bind a runtime method's call only to a member that scope
  reaches](./adr/3.40-bind-a-runtime-method-s-call-only-to-a-member-that-scope-reaches.md)
  - Governs: `kartta_lang::Conventions::runtime_methods`, `crates/kartta-graph/src/resolve.rs`, each language's
    `runtime.rs`
  - Revisit if: a language's extraction records the type a value holds for its member calls
- [3.41 Bind the caller's own owner only on the receivers a language
  lists](./adr/3.41-bind-the-caller-s-own-owner-only-on-the-receivers-a-language-lists.md)
  - Governs: `kartta_lang::Conventions::own_type_receivers`, `crates/kartta-graph/src/scope.rs`
    (`member_of_the_caller_s_owner`)
  - Revisit if: `parent::m()` and `super.m()` bind the base's member (#186)
- [3.42 Read transitive includes for a name few definitions
  share](./adr/3.42-read-transitive-includes-for-a-name-few-definitions-share.md)
  - Governs: `kartta_lang::IncludeReach`, `crates/kartta-graph/src/resolve.rs` (`through_includes`)
  - Revisit if: the C++ extraction reads `INLINE`-hidden templates and every class head
- [3.43 Guess no member near the caller for a C++ call on a
  value](./adr/3.43-guess-no-member-near-the-caller-for-a-c-plus-plus-call-on-a-value.md)
  - Governs: `kartta_lang::CallsOnValues`, `crates/kartta-graph/src/scope.rs` (`resolve_by_proximity`)
  - Revisit if: a sample taken after the declared-class receivers shows nearness right far more often than 15%
- [3.44 Bind no call to a namespace where a language's calls name
  none](./adr/3.44-bind-no-call-to-a-namespace-where-a-language-s-calls-name-none.md)
  - Governs: `kartta_lang::NamespaceCalls`, `crates/kartta-graph/src/resolve.rs`, `crates/kartta-graph/src/scope.rs`
  - Revisit if: a language that sets `Unbound` gains a construct through which a call names a namespace
- [3.45 Bind a call through a module's file as the module's paths see
  it](./adr/3.45-bind-a-call-through-a-module-s-file-as-the-module-s-paths-see-it.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`through_binding`, `module_export`, `macro_exported_below`,
    `glob_imported`), `kartta_lang::ImportBinding`
  - Revisit if: the extraction follows `pub use` (#217)
- [3.46 Bind no test definition by name for a call from outside test
  code](./adr/3.46-bind-no-test-definition-by-name-for-a-call-from-outside-test-code.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`can_bind_by_name`, `one_type_outside_tests`)
  - Revisit if: every language's test rule reads its test-support files and test directories in any case (#430, #432)
- [3.47 Bind a C++ call on a field through the class the field is declared
  as](./adr/3.47-bind-a-c-plus-plus-call-on-a-field-through-the-class-the-field-is-declared-as.md)
  - Governs: `crates/kartta-graph/src/scope.rs` (`member_of_a_reached_owner`, `class_of_own_field`,
    `nearest_base_fields`), `kartta_lang::Receivers`
  - Revisit if: a sample of the pairs the field tier binds finds the field's class wrong more than once in ten
- [4.1 Answer agents through an MCP server with the tools their prompts already
  name](./adr/4.1-answer-agents-through-an-mcp-server-with-the-tools-their-prompts-name.md) — initiative
  `04-consumer-surfaces`, 2026-10-18, revised 2026-10-23
- [4.2 Reach the engine through one JSON call over a small C
  ABI](./adr/4.2-reach-the-engine-through-one-json-call.md) — initiative `04-consumer-surfaces`, 2026-10-18, revised
  2026-10-23
- [4.3 Reach the engine from Node through an addon over the same
  dispatch](./adr/4.3-reach-the-engine-from-node-through-an-addon-over-the-same-dispatch.md) — initiative
  `04-consumer-surfaces`, 2026-10-18, revised 2026-10-22
- [4.4 Follow the indexed tree, and refresh it on
  request](./adr/4.4-follow-the-indexed-tree-and-refresh-it-on-request.md) — initiative `04-consumer-surfaces`,
  2026-10-18, revised 2026-10-22

## Deprecated

_None._

## Superseded

_None._
