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
  - Governs: `crates/kartta-fs/src/stamp.rs`, `crates/kartta-fs/src/index/refresh.rs` (`Previous::carry`),
    `BuildOptions::stamps`, `kartta_walk::WalkOptions::listed_stats`
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
  - Governs: `crates/kartta-fs/src/index.rs` (`RefreshOutcome::walked_over`, `RefreshOutcome::kept_positions`,
    `RefreshOutcome::position_after`, `RefreshOutcome::walked_positions`), `crates/kartta-fs/src/index/changed.rs`
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
  - Governs: `crates/kartta-fs/src/policy.rs` (`Payload`, `TargetId`, `Report::targets`),
    `crates/kartta-fs/src/index.rs` (`symlink_targets`)
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
  packs](./adr/2.1-share-per-file-data-through-content-addressed-packs.md)
  - Governs: `crates/kartta-cache/src/{key,pack,writer}.rs`, `kartta_cache::Store::{open, get, stats}`
  - Revisit if: a store has to sit on a filesystem without hard links, or cold builds are frequent enough that the
    shared writer's lock matters
- [2.2 Take per-file data from the shared cache in the walk's
  read](./adr/2.2-take-per-file-data-from-the-cache-in-the-walk-s-read.md)
  - Governs: `kartta_fs::BuildOptions::content_cache`, `kartta_fs::Content::{cached, cache}`,
    `crates/kartta-fs/src/cache.rs`, `crates/kartta-fs/src/policy.rs` (`stream_text`, `stream_cached`),
    `crates/kartta-fs/src/trigram.rs` (`fingerprint`, `encode`)
  - Revisit if: the content index's per-build sort and merge limit the nodes, or a hit's hashing nears the scan it skips
- [2.3 Expire cache entries by the time each was
  published](./adr/2.3-expire-cache-entries-by-the-time-each-was-published.md)
  - Governs: `crates/kartta-cache/src/pack.rs` (`Published`, `expired_before`), `kartta_cache::Store::merge`,
    `kartta_cache::DEFAULT_TTL`, `kartta_fs::BuildOptions::content_cache_ttl`
  - Revisit if: a store fills its disk within one time to live, or rewriting a pack past half expired costs a build
    more than recomputing what it held
- [2.4 Merge a cache's packs once they pass eight](./adr/2.4-merge-a-cache-s-packs-once-they-pass-eight.md)
  - Governs: `kartta_cache::Store::merge`, `kartta_cache::Merged`, `crates/kartta-cache/src/store.rs` (`merge_inputs`,
    `open_listed`), `crates/kartta-cache/src/pack.rs` (`ranges_tile_the_index`)
  - Revisit if: the merges of many processes finishing at once cost more than a lock on merging would
- [2.5 Remove what killed writers and older formats leave in a
  cache](./adr/2.5-remove-what-killed-writers-and-older-formats-leave-in-a-cache.md)
  - Governs: `crates/kartta-cache/src/store.rs` (`sweep`, `is_being_written`, `written_before`),
    `crates/kartta-cache/src/writer.rs` (`Writing::start`), `kartta_cache::Merged` (`orphans`, `orphans_left`,
    `unreadable`, `unreadable_left`, `left_in_place`), `crates/kartta-fs/src/cache.rs` (`unremoved`)
  - Revisit if: a cache directory sits on a filesystem without file locks while its builds stay idle for an hour
- [3.1 Plug languages in through a trait](./adr/3.1-plug-languages-in-through-a-trait.md)
  - Governs: `crates/kartta-lang/src/language.rs` (`Language`, `Resolver`, `Repository`, `Conventions`),
    `crates/kartta-lang/src/lib.rs`, `kartta_graph::Graph::conventions`
  - Revisit if: a host needs languages it was not compiled with
- [3.2 Name a Go call's target only where the syntax
  spells it](./adr/3.2-name-a-go-call-s-target-only-where-the-syntax-spells-it.md)
  - Governs: `crates/kartta-lang-go/src/walk/calls.rs`, `crates/kartta-lang-go/src/walk.rs` (`push_value_callers`,
    `type_spec`)
  - Revisit if: a Go file's walk can see its package's other files
- [3.3 Repair what the C++ grammar cannot parse instead of vendoring
  it](./adr/3.3-repair-what-the-c-plus-plus-grammar-cannot-parse.md)
  - Governs: `crates/kartta-lang-cpp/src/repair.rs`, `crates/kartta-lang-cpp/src/lib.rs` (`extract`, `EXTENSIONS`),
    `crates/kartta-lang-cpp/Cargo.toml` (`tree-sitter-cpp`)
  - Revisit if: `tree-sitter-cpp` publishes a release, or the share of files with a parse error grows on a new code base
- [3.4 Extract TypeScript and JavaScript as one family](./adr/3.4-extract-typescript-and-javascript-as-one-family.md)
  - Governs: `crates/kartta-lang-jsts/src/{walk,syntax,generated,flavor,lib}.rs`,
    `crates/kartta-lang-jsts/src/walk/{calls,exports}.rs`
  - Revisit if: a consumer needs the export surface of CommonJS modules
- [3.5 Keep the graph a file at a time](./adr/3.5-keep-the-graph-a-file-at-a-time.md)
  - Governs: `crates/kartta-graph/src/graph.rs` (`Graph::insert`, `Graph::remove`, `Graph::outline`,
    `Graph::definitions`, `find_test_modules`), `crates/kartta-graph/src/strings.rs`, `kartta_lang::Extraction`
    (`test_includes`)
  - Revisit if: a refresh's leftover strings or the name index's per-name lists are a measurable share of a graph
- [3.6 Build the graph inside the index's walk](./adr/3.6-build-the-graph-inside-the-index-s-walk.md)
  - Governs: `crates/kartta/src/extracting.rs` (`Extracting`, `claimant`), `crates/kartta/src/resolving.rs`
    (`Resolving::graphs`, `Resolving::insert_all`), `kartta::Engine::build`
  - Revisit if: decoding grows well past a tenth of a build's CPU, or the build hosts stop being bound by memory
- [3.7 Resolve calls when a query asks](./adr/3.7-resolve-calls-when-a-query-asks.md)
  - Governs: `crates/kartta-graph/src/{resolve,scope,calls,candidates,confidence}.rs`, `kartta_lang::Conventions`
  - Revisit if: a lazy `callers` of one name exceeds a second, or agents repeat names often enough that a memo pays for
    its invalidation
- [3.8 Extract only the daslang a file opts into gen2](./adr/3.8-extract-only-the-daslang-a-file-opts-into-gen2.md)
  - Governs: `crates/kartta-lang-daslang/src/walk.rs` (`opts_into_gen2`)
  - Revisit if: first-generation files appear in reviews as often as second-generation ones
- [3.9 Resolve a Quirrel import through the nearest
  .sqconfig](./adr/3.9-resolve-a-quirrel-import-through-the-nearest-sqconfig.md)
  - Governs: `crates/kartta-lang-quirrel/src/{resolve,sqconfig}.rs`, `crates/kartta-lang-quirrel/src/walk.rs`
    (`BANNER`)
  - Revisit if: a script tree mounts its roots somewhere other than a `.sqconfig`
- [3.10 Resolve a BLK include as the engine loads it](./adr/3.10-resolve-a-blk-include-as-the-engine-loads-it.md)
  - Governs: `crates/kartta-lang-blk/src/{lib,text,walk,resolve}.rs`
  - Revisit if: the engine's `DataBlock` reader changes how it reads an include's path
- [3.11 Name Rust items by their module chain and resolve modules by Cargo's
  layout](./adr/3.11-name-rust-items-by-their-module-chain-and-resolve-modules-by-cargo-s-layout.md)
  - Governs: `crates/kartta-lang-rust/src/{walk,resolve,lib}.rs`,
    `crates/kartta-lang-rust/src/walk/{settle,attributes}.rs`
  - Revisit if: a consumer needs the targets that a manifest places off Cargo's layout
- [3.12 Qualify Python by the module its path names](./adr/3.12-qualify-python-by-the-module-its-path-names.md)
  - Governs: `crates/kartta-lang-python/src/{resolve,walk,banner,lib}.rs`
  - Revisit if: an import root that the probe misses, or a root-first match that is wrong, shows up on a real tree
- [3.13 Read a shell script's commands as calls and its sources as
  includes](./adr/3.13-read-a-shell-script-s-commands-as-calls-and-its-sources-as-includes.md)
  - Governs: `crates/kartta-lang-bash/src/walk.rs`, `crates/kartta-lang-bash/src/resolve.rs`
  - Revisit if: a checkout shows sourced siblings that fail to resolve because their tails are ambiguous
- [3.14 Fold a PHP include path and map a use through
  PSR-4](./adr/3.14-fold-a-php-include-path-and-map-a-use-through-psr-4.md)
  - Governs: `crates/kartta-lang-php/src/{fold,resolve,psr4}.rs`, `crates/kartta-lang-php/src/walk.rs` (`include`,
    `namespace_use`)
  - Revisit if: the extraction reads PSR-0, `classmap` or `files` autoload, or the constants a project defines for its
    include paths
- [3.15 Qualify a Ruby member by its whole namespace
  chain](./adr/3.15-qualify-a-ruby-member-by-its-whole-namespace-chain.md)
  - Governs: `crates/kartta-lang-ruby/src/{owner,macros,bindings,locals}.rs`, `crates/kartta-lang-ruby/src/walk.rs`
    (definitions, receivers, bare names)
  - Revisit if: the extraction runs code, or reads what a type gains at run time through `define_method` or
    `class_eval`
- [3.16 Load a Lua module only from a literal name](./adr/3.16-load-a-lua-module-only-from-a-literal-name.md)
  - Governs: `crates/kartta-lang-lua/src/walk.rs`, `crates/kartta-lang-lua/src/resolve.rs`
  - Revisit if: the extraction reads a `.rockspec` or a `.luarc.json`, which name the module directories of a project
- [3.17 Resolve a Puppet class through the autoloader's
  layout](./adr/3.17-resolve-a-puppet-class-through-the-autoloader-s-layout.md)
  - Governs: `crates/kartta-lang-puppet/src/walk.rs`, `crates/kartta-lang-puppet/src/class.rs`,
    `crates/kartta-lang-puppet/src/resolve.rs`
  - Revisit if: the extraction reads `environment.conf` or a `Puppetfile`, which name the module path
- [3.18 Extract DSHL as one namespace that its includes
  join](./adr/3.18-extract-dshl-as-one-namespace-its-includes-join.md)
  - Governs: `crates/kartta-lang-dshl/src/{lib,walk}.rs`
  - Revisit if: a DSHL use is measured under a use tier that reads includes
- [3.19 Find the files that include a file through an index of
  includers](./adr/3.19-find-the-files-that-include-a-file-through-an-index-of-includers.md)
  - Governs: `crates/kartta-graph/src/dependents.rs` (`Graph::dependents`, `Dependents`),
    `crates/kartta-graph/src/graph.rs` (`included_by`, `includers_of`)
  - Revisit if: the include edges of a family make the index more than a few percent of its graph
- [3.20 Resolve implementors from the parents written when a query
  asks](./adr/3.20-resolve-implementors-from-the-parents-written-when-a-query-asks.md)
  - Governs: `crates/kartta-graph/src/implementations.rs`, `Graph::implementations`, `kartta_graph::is_parent`
  - Revisit if: one query exceeds tens of milliseconds, or the C++ extraction qualifies nested types by their whole
    chain or reads using-directives
- [3.21 Search definitions by the words of their names and
  paths](./adr/3.21-search-definitions-by-the-words-of-their-names-and-paths.md)
  - Governs: `crates/kartta-graph/src/{search,words}.rs`, `Graph::search`
  - Revisit if: a family's first search outgrows a query's latency, or refreshes make re-splitting cost more than
    sets kept at insert
- [3.22 Analyze what a change reaches with the walk the call queries
  take](./adr/3.22-analyze-what-a-change-reaches-with-the-walk-the-call-queries-take.md)
  - Governs: `crates/kartta-graph/src/{analyze,change}.rs`, `Graph::analyze_segment`, `Graph::analyze_change`,
    `Change::merge`, `ReachBand`, `Engine::analyze_change`
  - Revisit if: a large header must be analyzed faster than 0.3 s, or the per-query memo outgrows a few megabytes
- [3.23 Judge code dead only where the graph sees it
  used](./adr/3.23-judge-code-dead-only-where-the-graph-sees-it-used.md)
  - Governs: `crates/kartta-graph/src/dead.rs`, `Graph::dead_code`, `crates/kartta-graph/src/calls.rs`
    (`is_called_uncertainly`), `SymbolFlags::is_fallback`
  - Revisit if: a sample of the dead list finds most of it used through a form no extraction records
- [3.24 Count what a graph holds and how its calls bind when
  asked](./adr/3.24-count-what-a-graph-holds-and-how-its-calls-bind-when-asked.md)
  - Governs: `crates/kartta-graph/src/{stats,resolution}.rs`, `crates/kartta/src/{stats,extracting}.rs`,
    `Engine::stats`, `Engine::resolution`
  - Revisit if: a consumer asks for the edge count often enough that a whole resolution matters on each call
- [3.25 Read a definition's text and doc comment from its file when
  asked](./adr/3.25-read-a-definition-s-text-and-doc-comment-from-its-file-when-asked.md)
  - Governs: `crates/kartta/src/source.rs`, `Engine::sources`, `SourceOptions`, `Source`
  - Revisit if: a consumer asks for text often enough that reading the file costs more than keeping it
- [3.29 Name a Go receiver by the type its declarations
  spell](./adr/3.29-name-a-go-receiver-by-the-type-its-declarations-spell.md)
  - Governs: `crates/kartta-lang-go/src/walk/types.rs`, `crates/kartta-lang-go/src/walk/calls.rs`
    (`spelled_receiver`), `crates/kartta-graph/src/typed.rs`, `kartta_lang::Receivers::TypePaths`,
    `Symbol::value_types`
  - Revisit if: a defined type records its underlying type, or a language other than Go spells its receivers as paths
- [3.30 Look a repository path up without ASCII case through a table built when first
  asked](./adr/3.30-look-a-repository-path-up-without-case-through-a-table-built-when-first-asked.md)
  - Governs: `kartta_lang::Repository::files_ignoring_case`, `crates/kartta/src/repository.rs` (`FoldedPaths`,
    `folded_hash`, `IndexRepository::files_ignoring_case`), `crates/kartta-lang-blk/src/resolve.rs` (`indexed`),
    `crates/kartta-lang-quirrel/src/resolve.rs` (`loaded`)
  - Revisit if: the BLK and Quirrel resolvers' misses show in a build's time, or a third language's loader ignores case
- [3.31 Parse a C or C++ table without its numbers, and hold one tree at a
  time](./adr/3.31-parse-a-c-or-c-plus-plus-table-without-its-numbers.md)
  - Governs: `crates/kartta-lang-cpp/src/repair.rs` (`for_parse`, `scan_data`, `DATA_GROUP_MIN`),
    `crates/kartta-lang-cpp/src/lib.rs` (`extract`)
  - Revisit if: a served tree peaks again at the parse tree of a table that spells names outside a generated parser
- [3.32 Hand an engine every shipped language from one crate, and select them by
  name](./adr/3.32-hand-an-engine-every-shipped-language-from-one-crate.md)
  - Governs: `crates/kartta-languages/src/lib.rs` (`all`, `select`, `NAMES`, `UnknownLanguages`)
  - Revisit if: a consumer needs a build without some grammars, or two shipped languages claim the same file
- [3.33 Follow a tree through the questions each file's resolution
  asked](./adr/3.33-follow-a-tree-through-the-questions-each-file-s-resolution-asked.md)
  - Governs: `crates/kartta/src/{following,repository,resolving}.rs`, `kartta::Engine::refresh`,
    `kartta::Engine::refresh_paths`, `kartta::EngineRefresh`, `kartta_lang::Resolver`
  - Revisit if: the files a change reaches grow to a large share of the tree, past what holding the raw extractions
    would cost
- [3.34 Record each read and write of a field or a value, and bind it when a query
  asks](./adr/3.34-record-each-read-and-write-of-a-field-or-a-value.md)
  - Governs: `kartta_lang::{UseRef, Receiver, Access, DataUses}`, `crates/kartta-graph/src/{resolve,calls}.rs`,
    `Graph::references`
  - Revisit if: binding a name's uses when asked grows past the answer's budget
- [3.35 Pick a shared name's symbol by its path, and list the
  others](./adr/3.35-pick-a-shared-name-s-symbol-by-its-path-and-list-the-others.md)
  - Governs: `kartta_graph::Selector`, `kartta_graph::Candidate`, `is_referent`, `is_parent`,
    `crates/kartta-graph/src/{calls,implementations}.rs` (`select`, `passed_over`)
  - Revisit if: agents often pick a copy past the fifth candidate
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
  - Governs: `kartta_lang::MemberCalls`, `crates/kartta-graph/src/scope.rs` (`shape`, `takes_a_member_alone`,
    `through_a_module`, `path_names`, `receiver_names_a_module`, `one_callable`, `resolve_by_proximity`),
    `crates/kartta-graph/src/resolve.rs` (`resolve_by_name`), `crates/kartta-lang-go/src/lib.rs`,
    `crates/kartta-lang-rust/src/lib.rs`
  - Revisit if: the C++ extraction reads every class head, so no member reaches the graph as a free function
- [3.40 Bind a runtime method's call only to a member that scope
  reaches](./adr/3.40-bind-a-runtime-method-s-call-only-to-a-member-that-scope-reaches.md)
  - Governs: `kartta_lang::Conventions::runtime_methods`, `kartta_lang::IncludedRuntimeMembers`,
    `crates/kartta-graph/src/resolve.rs` (`is_runtime_method`, `resolve_by_name`, `resolve_outside_scope`, `offered`,
    `spelled_or_own`), `crates/kartta-graph/src/scope.rs` (`receiver_spells_owner`, `owner_derives_from`), the
    `runtime.rs` of `kartta-lang-jsts`, `kartta-lang-quirrel`, `kartta-lang-python` and `kartta-lang-rust`,
    `crates/kartta-lang-quirrel/src/lib.rs` (`CONVENTIONS`)
  - Revisit if: a language's extraction records the type a value holds for its member calls
- [3.41 Bind the caller's own owner only on the receivers a language
  lists](./adr/3.41-bind-the-caller-s-own-owner-only-on-the-receivers-a-language-lists.md)
  - Governs: `kartta_lang::Conventions::own_type_receivers`, `crates/kartta-graph/src/scope.rs`
    (`member_of_the_caller_s_owner`)
  - Revisit if: a list of PHP receivers reaches 90% on a sample taken with `parent::m()` out of the tier
- [3.42 Read transitive includes for a name few definitions
  share](./adr/3.42-read-transitive-includes-for-a-name-few-definitions-share.md)
  - Governs: `kartta_lang::IncludeReach`, `crates/kartta-graph/src/resolve.rs` (`included`, `through_includes`,
    `closure`, `THROUGH_INCLUDES_MAX_DEFINITIONS`, `is_pasted_where_called`), `crates/kartta-lang-cpp/src/lib.rs`
  - Revisit if: the C++ extraction reads `INLINE`-hidden templates and every class head
- [3.43 Guess no member near the caller for a C++ call on a
  value](./adr/3.43-guess-no-member-near-the-caller-for-a-c-plus-plus-call-on-a-value.md)
  - Governs: `kartta_lang::CallsOnValues`, `crates/kartta-graph/src/scope.rs` (`resolve_by_proximity`),
    `crates/kartta-lang-cpp/src/lib.rs`
  - Revisit if: a sample taken after the declared-class receivers shows nearness right far more often than 15%
- [3.44 Bind no call to a namespace where a language's calls name
  none](./adr/3.44-bind-no-call-to-a-namespace-where-a-language-s-calls-name-none.md)
  - Governs: `kartta_lang::NamespaceCalls`, `crates/kartta-graph/src/resolve.rs` (`is_unbound_namespace`,
    `resolve_call`), `crates/kartta-graph/src/scope.rs` (`admits`), `crates/kartta-lang-rust/src/lib.rs`
  - Revisit if: a language that sets `Unbound` gains a construct through which a call names a namespace
- [3.45 Bind a call through a module's file as the module's paths see
  it](./adr/3.45-bind-a-call-through-a-module-s-file-as-the-module-s-paths-see-it.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`through_binding`, `module_export`, `module_item`,
    `used_by_module`, `globbed_by_module`, `macro_exported_below`, `glob_imported`), `kartta_lang::ImportBinding`
  - Revisit if: a miss past a module's file, its bindings, its globs and the macros exported below it leaves only
    inline modules, `#[path]` modules and crates outside the tree
- [3.46 Bind no test definition by name for a call from outside test
  code](./adr/3.46-bind-no-test-definition-by-name-for-a-call-from-outside-test-code.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`can_bind_by_name`, `one_type_outside_tests`,
    `resolve_outside_scope`), `crates/kartta-graph/src/confidence.rs` (`UNIQUE`, `SAME_DIR`)
  - Revisit if: every language's test rule reads its test-support files (#432)
- [3.47 Bind a C++ call on a field through the class the field is declared
  as](./adr/3.47-bind-a-c-plus-plus-call-on-a-field-through-the-class-the-field-is-declared-as.md)
  - Governs: `crates/kartta-graph/src/scope.rs` (`member_of_a_reached_owner`, `class_of_own_field`,
    `nearest_base_fields`), `kartta_lang::Receivers`
  - Revisit if: a sample of the pairs the field tier binds finds the field's class wrong more than once in ten
- [3.48 Read a qualified name with the other owner separator when it selects
  nothing](./adr/3.48-read-a-qualified-name-with-the-other-owner-separator-when-it-selects-nothing.md)
  - Governs: `crates/kartta/src/reading.rs` (`ask_reading`), `crates/kartta-mcp/src/server.rs`, the name parameters
    of `crates/kartta-mcp/src/params.rs`
  - Revisit if: a shipped language mixes `.` and `::` within its qualified names
- [3.49 Select a name whose last part holds a separator by its longer
  parts](./adr/3.49-select-a-name-whose-last-part-holds-a-separator-by-its-longer-parts.md)
  - Governs: `crates/kartta-graph/src/graph.rs` (`Graph::definitions`)
  - Revisit if: a miss's longer-part lookups show in a profile of a symbol query
- [3.50 Bind a Rust path into another crate through its library's module
  files](./adr/3.50-bind-a-rust-path-into-another-crate-through-its-library-s-module-files.md)
  - Governs: `crates/kartta-lang-rust/src/walk/imports.rs`, `crates/kartta-lang-rust/src/manifest.rs`,
    `crates/kartta-lang-rust/src/resolve.rs` (`Crates`)
  - Revisit if: the call record carries a path per call (#236)
- [3.51 Start a Rust path through `crate::`, `self::` or `super::` at the caller's crate root or
  module](./adr/3.51-start-a-rust-path-through-crate-self-or-super-at-the-caller-s-crate-root-or-module.md)
  - Governs: `crates/kartta-lang-rust/src/walk/imports.rs` (`starts_inside_the_file`, `start_at_declared_modules`,
    `reach_own_names_through_their_module`, `kept_paths`, `module_files`), `crates/kartta-lang-rust/src/walk/calls.rs`
    (`record_call`), `crates/kartta-lang-rust/src/resolve.rs` (`Crates::crate_root`, `Caller`)
  - Revisit if: the extraction places each inline module in its file's module tree
- [3.52 Scope a Rust `use` inside a block to that block](./adr/3.52-scope-a-rust-use-inside-a-block-to-that-block.md)
  - Governs: `crates/kartta-lang-rust/src/walk/imports.rs`, `crates/kartta-lang-rust/src/walk/blocks.rs`
  - Revisit if: the call record carries a path per call (#236)
- [3.53 Record no Rust call through a value, and bind a call of a body's item as a local
  call](./adr/3.53-record-no-rust-call-through-a-value-and-bind-a-body-s-item-as-a-local-call.md)
  - Governs: `crates/kartta-lang-rust/src/walk/{blocks,bindings,calls}.rs`
  - Revisit if: a consumer needs the callable that a closure or a function pointer holds
- [3.54 Read the calls a Rust macro's arguments write from its
  tokens](./adr/3.54-read-the-calls-a-rust-macro-s-arguments-write-from-its-tokens.md)
  - Governs: `crates/kartta-lang-rust/src/walk/arguments.rs`
  - Revisit if: tree-sitter-rust parses a macro's arguments as expressions
- [3.55 Name a Rust method call's receiver by its parameter's declared
  type](./adr/3.55-name-a-rust-method-call-s-receiver-by-its-parameter-s-declared-type.md)
  - Governs: `crates/kartta-lang-rust/src/walk/declared.rs`, `crates/kartta-lang-rust/src/walk/bindings.rs`
    (`declared_type`), `crates/kartta-lang-rust/src/walk/calls.rs` (`named_target`)
  - Revisit if: the owner tier tells crates apart (#202)
- [3.56 Name a Rust call's receiver only where its syntax spells
  it](./adr/3.56-name-a-rust-call-s-receiver-only-where-its-syntax-spells-it.md)
  - Governs: `crates/kartta-lang-rust/src/walk/calls.rs`, `crates/kartta-lang-rust/src/walk/settle.rs`
  - Revisit if: the walk learns the type a local, a field or a chain holds (#484, #248, #485)
- [3.60 Record every Go data site its walk can place](./adr/3.60-record-every-go-data-site-its-walk-can-place.md)
  - Governs: `crates/kartta-lang-go/src/walk/uses.rs`, `crates/kartta-lang-go/src/walk/syntax.rs`, the Go
    `Conventions`, `SymbolFlags::is_reflected`
  - Revisit if: a Go file's walk can see its package's other files
- [3.62 Record TypeScript and JavaScript data sites through what a value
  declares](./adr/3.62-record-typescript-and-javascript-data-sites-through-what-a-value-declares.md)
  - Governs: `crates/kartta-lang-jsts/src/walk.rs`, `crates/kartta-lang-jsts/src/walk/uses.rs`,
    `crates/kartta-lang-jsts/src/walk/context.rs`, the TS/JS `Conventions`
  - Revisit if: the extraction records the type a value holds where no declaration spells it
- [3.63 Record Python data sites in the scope that binds
  them](./adr/3.63-record-python-data-sites-in-the-scope-that-binds-them.md)
  - Governs: `crates/kartta-lang-python/src/walk.rs`, `crates/kartta-lang-python/src/walk/uses.rs`, the Python
    `Conventions`
  - Revisit if: the file's one field stops binding right for most Python values
- [3.64 Record C++ data sites and bind them as class-scope lookup
  does](./adr/3.64-record-c-plus-plus-data-sites-and-bind-them-as-class-scope-lookup-does.md)
  - Governs: `crates/kartta-lang-cpp/src/walk/sites.rs`, `crates/kartta-lang-cpp/src/walk.rs`, the C++ `Conventions`,
    `crates/kartta-graph/src/{members,resolve}.rs`, `crates/kartta-mcp/src/render/calls.rs`
  - Revisit if: the extraction parses every class head and the operand's type where no declaration spells it
- [3.65 Tell C++ namesake classes apart by the scopes a use
  writes](./adr/3.65-tell-c-plus-plus-namesake-classes-apart-by-the-scopes-a-use-writes.md)
  - Governs: `crates/kartta-graph/src/{twins,members}.rs`, `crates/kartta-lang-cpp/src/walk/sites.rs`
  - Revisit if: the extraction keeps a template's arguments or the vendored copy a file belongs to
- [3.66 Record a name a script or Rust reads without a call as a
  use](./adr/3.66-record-a-name-a-script-or-rust-reads-without-a-call-as-a-use.md)
  - Governs: the walks of `crates/kartta-lang-{daslang,quirrel,lua}`,
    `crates/kartta-lang-rust/src/walk/{values,bindings}.rs`
  - Revisit if: a language here gains imports the walk follows
- [3.67 Record a function a string or a symbol names as a
  use](./adr/3.67-record-a-function-a-string-or-a-symbol-names-as-a-use.md)
  - Governs: the walks of `crates/kartta-lang-{php,ruby,bash}`
  - Revisit if: a corpus with Rails' callbacks shows them as a share of the dead answers worth reading
- [3.68 Keep a use in 16 bytes and spell its text once a
  file](./adr/3.68-keep-a-use-in-16-bytes-and-spell-its-text-once-a-file.md)
  - Governs: `crates/kartta-lang/src/codec.rs`, `crates/kartta-graph/src/{stored,graph}.rs`, `kartta_lang::FINGERPRINT`
  - Revisit if: a use needs more than four receiver forms and three accesses, or a file holds 2^28 definitions
- [3.70 Read a declarator a macro misjoins in the walk](./adr/3.70-read-a-declarator-a-macro-misjoins-in-the-walk.md)
  - Governs: `crates/kartta-lang-cpp/src/walk/declarator.rs`, `crates/kartta-lang-cpp/src/walk.rs`
    (`misread_definition_body`)
  - Revisit if: the grammar reads an annotation macro as an attribute, or a tree-sitter release recovers differently
- [3.71 Read a conditional that splits a brace with its first branch
  taken](./adr/3.71-read-a-conditional-that-splits-a-brace-with-its-first-branch-taken.md)
  - Governs: `crates/kartta-lang-cpp/src/repair.rs` (`untaken_branches`, `ConditionalScan`, `BraceSplit`,
    `recovery_of`)
  - Revisit if: a tree-sitter-cpp release reads a conditional's branches as alternatives
- [3.72 Qualify C++ names the way the graph matches
  them](./adr/3.72-qualify-c-plus-plus-names-the-way-the-graph-matches-them.md)
  - Governs: `crates/kartta-lang-cpp/src/walk.rs` (`apply_namespace`, `attribute_to_class`, `nest_in_class`,
    `free_namespace_members`), `crates/kartta-lang-cpp/src/walk/name.rs` (`call_target`)
  - Revisit if: the graph matches a call by a C++ scope chain rather than by a bare owner and a qualified name
- [3.73 Resolve a C++ include to the copy nearest the including
  file](./adr/3.73-resolve-a-c-plus-plus-include-to-the-copy-nearest-the-including-file.md)
  - Governs: `crates/kartta-lang-cpp/src/resolve.rs` (`include`, `enclosing_copy`, `nearest_copy`)
  - Revisit if: kartta reads a build's include path
- [3.74 Fold a C++ declaration into the one definition its file
  reaches](./adr/3.74-fold-a-c-plus-plus-declaration-into-the-one-definition-its-file-reaches.md)
  - Governs: `crates/kartta-lang-cpp/src/walk.rs` (`queue_prototype`, `flush_prototypes`),
    `crates/kartta-lang-cpp/src/resolve.rs` (`export_c_constants`), `crates/kartta-graph/src/resolve.rs`
    (`linked_definition`)
  - Revisit if: several definitions a header reaches through transitive includes are not alternatives
- [3.75 Name a C++ call's receiver by the class its operand is declared
  as](./adr/3.75-name-a-c-plus-plus-call-s-receiver-by-the-class-its-operand-is-declared-as.md)
  - Governs: `crates/kartta-lang-cpp/src/walk.rs` (`record_call`, `push_fields`),
    `crates/kartta-lang-cpp/src/walk/{sites,data}.rs`, `crates/kartta-graph/src/scope.rs`
    (`member_of_a_reached_owner`), `crates/kartta-graph/src/twins.rs`
  - Revisit if: calls on operands the walk does not see declared bind wrongly often enough to want a typed expression
- [3.76 Count a generated-code marker only outside a code
  span](./adr/3.76-count-a-generated-code-marker-only-outside-a-code-span.md)
  - Governs: `crates/kartta-lang/src/banner.rs` (`kartta_lang::Banner`)
  - Revisit if: a generator in the trees kartta serves writes its banner inside backticks
- [3.80 Widen a named refresh to the whole tree past one part for every 30
  entries](./adr/3.80-widen-a-named-refresh-to-the-whole-tree-past-one-part-for-every-30-entries.md)
  - Governs: `crates/kartta/src/engine.rs` (`refresh_parts`), `kartta::Engine::refresh_paths_limit`,
    `kartta::EngineRefresh::widened`, `kartta::Widened`
  - Revisit if: the index takes the parts themselves on its threads, or a followed tree's whole refresh costs far more
    an entry, as on a network mount
- [3.81 Find the files a refresh reaches by one pass over a buffer of their
  keys](./adr/3.81-find-the-files-a-refresh-reaches-by-one-pass-over-a-buffer-of-their-keys.md)
  - Governs: `crates/kartta/src/following.rs` (`Asked`, `Reached`)
  - Revisit if: the pass is again a large part of a refresh a consumer waits on
- [3.82 Build a followed family's resolver again only when a refresh reaches what it
  read](./adr/3.82-build-a-followed-family-s-resolver-again-only-when-a-refresh-reaches-what-it-read.md)
  - Governs: `crates/kartta/src/following.rs` (`Following::plan`, `Following::reconfigure`, `reaches`),
    `crates/kartta/src/resolving.rs` (`Resolvers`), `crates/kartta/src/repository.rs` (`Configuration`,
    `configured_while`), `kartta::EngineRefresh::reconfigured`, `kartta_lang::Resolver::resolves_as`, the `resolves_as`
    of `crates/kartta-lang-{go,blk,jsts,quirrel,rust}/src/resolve.rs`
  - Revisit if: most edits to a configuration change what its resolver derives, or a kept resolver grows to a share of
    the heap
- [3.83 Keep the tables a followed engine's resolvers find files in across
  refreshes](./adr/3.83-keep-the-tables-a-followed-engine-s-resolvers-find-files-in-across-refreshes.md)
  - Governs: `crates/kartta/src/repository.rs` (`FileTables`, `IndexRepository::recording`,
    `IndexRepository::into_tables`), `crates/kartta/src/following.rs` (`Following::refresh`)
  - Revisit if: most refreshes stop asking by name
- [3.100 Resolve a TypeScript or JavaScript import as the compiler and bundlers
  do](./adr/3.100-resolve-a-typescript-or-javascript-import-as-the-compiler-and-bundlers-do.md)
  - Governs: `crates/kartta-lang-jsts/src/{resolve,modules,tsconfig,package_json,config}.rs`,
    `crates/kartta-lang-jsts/src/walk/exports.rs` (`push_directive`, `imported_names`)
  - Revisit if: a real tree's imports lead through configuration the resolver does not read, a bundler's aliases or a
    web server's root (#424)
- [3.101 Name no type for a TypeScript or JavaScript call on a value the file
  declares](./adr/3.101-name-no-type-for-a-typescript-or-javascript-call-on-a-value-the-file-declares.md)
  - Governs: `crates/kartta-lang-jsts/src/walk.rs` (`Binding`, `Declared`), `crates/kartta-lang-jsts/src/walk/calls.rs`
    (`value_receiver`, `this_class`), `crates/kartta-lang-jsts/src/walk/context.rs`
  - Revisit if: the walk learns the type a value's declaration spells for its calls (#475)
- [3.102 Bind a Python bare call by the scope that binds its
  name](./adr/3.102-bind-a-python-bare-call-by-the-scope-that-binds-its-name.md)
  - Governs: `crates/kartta-lang-python/src/walk.rs` (`Frame`, `Binding`, `close`, `scope_frame`,
    `drop_builtin_calls`), `crates/kartta-lang-python/src/runtime.rs` (`BUILTINS`)
  - Revisit if: a real tree shows the stronger binding of a scope wrong at the call often enough to need an order of
    execution
- [3.103 Declare a Python class's fields from its body and its instance's
  attributes](./adr/3.103-declare-a-python-class-s-fields-from-its-body-and-its-instance-s-attributes.md)
  - Governs: `crates/kartta-lang-python/src/walk.rs` (`assignment`, `OwnReceiver`, `attribute_assignment`,
    `declare_attributes`)
  - Revisit if: a consumer needs a field to tell declared data from state a method keeps
- [3.110 Judge a language's data only where it records every read and
  write](./adr/3.110-judge-a-language-s-data-only-where-it-records-every-read-and-write.md)
  - Governs: `kartta_lang::DataUses`, `crates/kartta-graph/src/{dead,calls}.rs`, `SymbolFlags::is_reflected`,
    `crates/kartta-lang-go/src/walk/syntax.rs`
  - Revisit if: a language besides Go records a use wherever code reads or writes its data
- [3.111 Mark what a runtime calls with no name as an entry
  point](./adr/3.111-mark-what-a-runtime-calls-with-no-name-as-an-entry-point.md)
  - Governs: `SymbolFlags::is_entry`, the walkers of `crates/kartta-lang-{cpp,python,rust,go,bash,daslang}`
  - Revisit if: a runtime calls definitions by a convention its extraction does not mark, often enough to fill the
    dead list
- [3.112 Keep the names a file's top level calls and
  uses](./adr/3.112-keep-the-names-a-file-s-top-level-calls-and-uses.md)
  - Governs: `crates/kartta-graph/src/graph.rs` (`store_file_scope_names`), `crates/kartta-graph/src/calls.rs`
    (`file_scope_callers`), `Walk::file_scope`, each language's walker
  - Revisit if: the file-scope names grow past a few percent of a build's heap, or an agent needs a file-scope call's
    line
- [3.120 Derive a language's fingerprint from its code and its dependencies'
  versions](./adr/3.120-derive-a-language-s-fingerprint-from-its-code-and-its-dependencies-versions.md)
  - Governs: `kartta_lang::Language::fingerprint`, `crates/kartta-lang/src/fingerprint.rs` (`set_source_id`),
    `crates/kartta-lang/src/tree.rs` (`fingerprint`, `tree_sitter_fingerprint!`), `kartta_lang::FINGERPRINT`, each
    language crate's `build.rs`
  - Revisit if: a language's extraction depends on something outside its sources, manifest and locked dependencies
- [3.121 State a language's conventions as a const over the
  default](./adr/3.121-state-a-language-s-conventions-as-a-const-over-the-default.md)
  - Governs: `kartta_lang::Conventions`, `Conventions::DEFAULT`, the `CONVENTIONS` of each
    `crates/kartta-lang-*/src/lib.rs`
  - Revisit if: a language needs conventions it can only work out at run time
- [3.122 Resolve a language family through the resolver its first language
  builds](./adr/3.122-resolve-a-language-family-through-the-resolver-its-first-language-builds.md)
  - Governs: `kartta_lang::Language::family`, `kartta_lang::Language::resolver`, `crates/kartta/src/resolving.rs`
    (`Resolving::resolvers`), `crates/kartta/src/engine.rs` (`families`)
  - Revisit if: a family's languages need different resolutions of the same import
- [3.123 Hand a resolver the repository each file resolves
  against](./adr/3.123-hand-a-resolver-the-repository-each-file-resolves-against.md)
  - Governs: `kartta_lang::Language::resolver`, `kartta_lang::Resolver::resolve`, each
    `crates/kartta-lang-*/src/resolve.rs`
  - Revisit if: the view a resolver builds inside `resolve` shows in a build's profile
- [3.124 Resolve a build's extractions 1,024 files at a
  time](./adr/3.124-resolve-a-build-s-extractions-1-024-files-at-a-time.md)
  - Governs: `crates/kartta/src/resolving.rs` (`RESOLVED_AT_ONCE`, `Resolving::insert_all`)
  - Revisit if: the call graph or a tree of uneven files moves the build's peak away from a 1,024-file batch
- [3.130 Name the type alias a parent's lookup
  reaches](./adr/3.130-name-the-type-alias-a-parent-s-lookup-reaches.md)
  - Governs: `crates/kartta-graph/src/implementations.rs`, `kartta_graph::is_parent`
  - Revisit if: the extraction records what an alias names
- [3.131 Answer a missed name with the definitions one edit from
  it](./adr/3.131-answer-a-missed-name-with-the-definitions-one-edit-from-it.md)
  - Governs: `crates/kartta-graph/src/one_edit.rs`, `Graph::definitions_one_edit_from`
  - Revisit if: the meant definition falls outside one edit for more than a few of 500 typos, or a miss outgrows tens
    of milliseconds at p99
- [3.132 Walk from a field, a variable or a constant to its
  users](./adr/3.132-walk-from-a-field-a-variable-or-a-constant-to-its-users.md)
  - Governs: `crates/kartta-graph/src/calls.rs` (`dependents_of`, `user_edges`), `SegmentSymbol::users`,
    `Seed::user_count`, `Seed::consumers`
  - Revisit if: a sample of the users a data symbol's walk takes finds more than one in ten wrong
- [3.133 Count a blast's files from its callers and its file's includers
  apart](./adr/3.133-count-a-blast-s-files-from-its-callers-and-its-file-s-includers-apart.md)
  - Governs: `crates/kartta-graph/src/analyze.rs` (`includers_within`, `Blast::affected_files`)
  - Revisit if: a consumer needs the affected files listed, not counted
- [3.134 Find a doc comment by the comment forms of its
  language](./adr/3.134-find-a-doc-comment-by-the-comment-forms-of-its-language.md)
  - Governs: `crates/kartta-lang/src/doc.rs`, `Language::doc_comments`, `DocComments`, each language's `doc_comments`
  - Revisit if: a language's extraction records comments
- [3.140 Find a daslang module by its file name](./adr/3.140-find-a-daslang-module-by-its-file-name.md)
  - Governs: `crates/kartta-lang-daslang/src/resolve.rs`, `crates/kartta-lang-daslang/src/walk.rs` (`require`,
    `include`), `crates/kartta-graph/src/resolve.rs` (`required_definition`)
  - Revisit if: `module` clauses that differ from their file stems pay for a pass over every file, or a project's
    `include_get` can be read
- [3.141 Give a daslang call to the code that runs it](./adr/3.141-give-a-daslang-call-to-the-code-that-runs-it.md)
  - Governs: `crates/kartta-lang-daslang/src/walk.rs` (`anonymous_scope`, `block_call`, `constructor`, `new_call`,
    `data_macro`, `data_arguments`, `data_block`, `splice`)
  - Revisit if: the daScript macro library adds a macro whose compiler class reads an argument as syntax
- [3.142 Take the one file that differs only in case where a game's vromfs loads
  it](./adr/3.142-take-the-one-file-that-differs-only-in-case-where-a-game-s-vromfs-loads-it.md)
  - Governs: `crates/kartta-lang-quirrel/src/resolve.rs` (`loaded`), `crates/kartta-lang-blk/src/resolve.rs`
    (`indexed`), `crates/kartta-lang-blk/src/mounts.rs` (`Mount::place`)
  - Revisit if: a Quirrel script no `.sqconfig` governs, or a BLK `#` include, names a file in another case alone
- [3.143 Name a Quirrel function value by its binding](./adr/3.143-name-a-quirrel-function-value-by-its-binding.md)
  - Governs: `crates/kartta-lang-quirrel/src/walk.rs` (`function_value`, `value_binding`, `module_table_name`,
    `data_field`, `class_name`, `assignment_target`, `is_squirrel_extends`)
  - Revisit if: importers reach a module's slots through the table rather than through a `from` import's bare names
- [3.144 Record no Quirrel call through a parameter or a
  local](./adr/3.144-record-no-quirrel-call-through-a-parameter-or-a-local.md)
  - Governs: `crates/kartta-lang-quirrel/src/walk.rs` (`call`, `bare_call`, `declared_names`, `finish`)
  - Revisit if: the walk learns which definition a parameter or a local holds
- [3.145 List the methods the game engine binds on Quirrel's native
  values](./adr/3.145-list-the-methods-the-game-engine-binds-on-quirrel-s-native-values.md)
  - Governs: `crates/kartta-lang-quirrel/src/runtime.rs`, `crates/kartta-lang-quirrel/src/lib.rs` (`CONVENTIONS`)
  - Revisit if: the Quirrel extraction records the type a value holds for its member calls
- [3.146 Read the BLK mount table from the files that state it by
  name](./adr/3.146-read-the-blk-mount-table-from-the-files-that-state-it-by-name.md)
  - Governs: `crates/kartta-lang-blk/src/mounts.rs`, `crates/kartta-lang-blk/src/resolve.rs` (`MOUNT_FILE_NAMES`),
    `crates/kartta-lang-blk/src/walk.rs` (`record_mounts`)
  - Revisit if: a resolver can see the extractions of the files its language claims, or a tree states the mounts its
    includes need in a file of another name
- [3.147 Resolve a DSHL include to the nearest file of its
  spelling](./adr/3.147-resolve-a-dshl-include-to-the-nearest-file-of-its-spelling.md)
  - Governs: `crates/kartta-lang-dshl/src/resolve.rs`, `crates/kartta-lang-dshl/src/walk.rs` (`include`,
    `hlsl_includes`)
  - Revisit if: the build configuration becomes an input of the graph, or the contract can mark edges as alternatives
- [3.150 Resolve a Go import to the package the go command
  builds](./adr/3.150-resolve-a-go-import-to-the-package-the-go-command-builds.md)
  - Governs: `crates/kartta-lang-go/src/resolve.rs`, `crates/kartta-graph/src/resolve.rs` (`read_bindings`,
    `in_package_of`, `is_external_test_of`, `in_tested_package`)
  - Revisit if: the indexed Go trees build through files the resolver does not read, a `go.work` or a replace by version
- [3.155 Own a PHP member by its type's bare name](./adr/3.155-own-a-php-member-by-its-type-s-bare-name.md)
  - Governs: `crates/kartta-lang-php/src/walk.rs` (qualification, `call_target`, `closure_binding`,
    `object_creation`), `crates/kartta-lang-php/src/lib.rs` (`Conventions`)
  - Revisit if: two classes of one name in different namespaces join their members on a real tree
- [3.156 Name a PHP anonymous class by the names that lead to
  it](./adr/3.156-name-a-php-anonymous-class-by-the-names-that-lead-to-it.md)
  - Governs: `crates/kartta-lang-php/src/walk.rs` (`anonymous_class`, `forget_holder`, `held_receiver`,
    `enclosing_type`)
  - Revisit if: a shorter name keeps a file's anonymous classes apart
- [3.157 Resolve a Ruby require under the load roots projects
  set](./adr/3.157-resolve-a-ruby-require-under-the-load-roots-projects-set.md)
  - Governs: `crates/kartta-lang-ruby/src/resolve.rs`, `crates/kartta-lang-ruby/src/walk.rs` (`require_form`,
    `first_string_argument`)
  - Revisit if: the extraction reads a `Gemfile` or the `$LOAD_PATH` additions of a project
- [3.158 Credit a Ruby call in a type's body to the
  type](./adr/3.158-credit-a-ruby-call-in-a-type-s-body-to-the-type.md)
  - Governs: `crates/kartta-lang-ruby/src/walk.rs` (`site_of`, `is_type_body`, `type_of_self`, `call`)
  - Revisit if: a sample of the pairs the type's receiver binds at 60 and above falls well under 85% right
- [3.165 Blank a generated tree-sitter parser's tables before the C
  parse](./adr/3.165-blank-a-generated-tree-sitter-parser-s-tables.md)
  - Governs: `crates/kartta-lang-cpp/src/repair.rs` (`Tables`, `GENERATED_PARSER_MARKS`, `data_token_len`)
  - Revisit if: a generated `parser.c` no longer includes `tree_sitter/parser.h` and defines `LANGUAGE_VERSION` before
    its first brace, or its tables spell a definition a graph holds
- [3.166 Break a Quirrel member call's tie by the tables its receiver
  reaches](./adr/3.166-break-a-quirrel-member-call-s-tie-by-the-tables-its-receiver-reaches.md)
  - Governs: `kartta_lang::UntypedOwners`, `crates/kartta-graph/src/scope.rs` (`scoped`, `one_owner_reached_by_name`,
    `spells`), `crates/kartta-lang-quirrel/src/lib.rs` (`CONVENTIONS`)
  - Revisit if: the Quirrel walk records the type a file-scope binding holds
- [3.167 Bind a PHP static call on a class outside the graph through imports
  alone](./adr/3.167-bind-a-php-static-call-on-a-class-outside-the-graph-through-imports-alone.md)
  - Governs: `kartta_lang::BareReceivers`, `crates/kartta-graph/src/resolve.rs` (`resolve_by_name`,
    `names_a_class_outside_the_graph`), `crates/kartta-lang-php/src/lib.rs` (`CONVENTIONS`)
  - Revisit if: the PHP extraction reads a `use … as` alias
- [3.168 Bind a call through the base receiver to the base class's
  member](./adr/3.168-bind-a-call-through-the-base-receiver-to-the-base-class-s-member.md)
  - Governs: `kartta_lang::Conventions::base_receivers`, `crates/kartta-graph/src/scope.rs`
    (`member_of_the_caller_s_base`, `held_by_a_base`, `base_types`), `crates/kartta-graph/src/resolve.rs`
    (`resolve_by_tiers`), the `Conventions` of `kartta-lang-quirrel`, `kartta-lang-daslang` and `kartta-lang-php`
  - Revisit if: the extraction records the class a base's alias names, or a language whose base receiver names a base
    by a qualified name lists one
- [3.169 Repair from the first C++ tree's errors before a second
  parse](./adr/3.169-repair-from-the-first-c-plus-plus-tree-s-errors-before-a-second-parse.md)
  - Governs: `crates/kartta-lang-cpp/src/repair.rs` (`after_recovery`, `recovery_of`, `macro_invocation_len`,
    `annotation_macro`, `label_before_an_endif`), `crates/kartta-lang-cpp/src/lib.rs` (`extract`)
  - Revisit if: a `tree-sitter-cpp` release moves where the first tree's errors fall, or the files parsed twice grow
    well past 4% of a tree's C++ files
- [3.170 Own a Go anonymous struct's fields by the field or the variable it
  types](./adr/3.170-own-a-go-anonymous-struct-s-fields-by-the-field-or-the-variable-it-types.md)
  - Governs: `crates/kartta-lang-go/src/walk/{anonymous,types}.rs`, `crates/kartta-graph/src/typed.rs`
  - Revisit if: a path is read outside the file that spells it
- [3.171 Bind a call through the import, type, qualified name or owner it is written
  on](./adr/3.171-bind-a-call-through-the-import-type-qualified-name-or-owner-it-is-written-on.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`resolve_by_tiers`, `through_binding`,
    `written_inside_an_enclosing_scope`, `nested_in_written_owner`), `crates/kartta-graph/src/scope.rs`
    (`member_of_a_reached_owner`, `member_of_the_receiver_s_owner`), `kartta_lang::Conventions` (`scope_separator`,
    `class_member_separator`)
  - Revisit if: a language's calls name another package's owner of a name about as often as their own package's
- [3.172 Bind a bare call through the scope that offers its name, then the nearest
  callable](./adr/3.172-bind-a-bare-call-through-the-scope-that-offers-its-name-then-the-nearest-callable.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`resolve_by_tiers`, `resolve_by_name`, `resolve_outside_scope`,
    `imports_from_outside`), `crates/kartta-graph/src/scope.rs` (`resolve_by_proximity`), `kartta_lang::Conventions`
    (`own_members`, `inherited_members`)
  - Revisit if: a language's scope finds a bare name through a step these tiers do not model, or ranks its steps in
    another order
- [3.173 Bind a use of a field or a value by what its receiver
  names](./adr/3.173-bind-a-use-of-a-field-or-a-value-by-what-its-receiver-names.md)
  - Governs: `crates/kartta-graph/src/resolve.rs` (`resolve_use`, `member_use`, `value_use`, `bare_use`,
    `imported_value`), `crates/kartta-graph/src/{members,typed}.rs`,
    `kartta_lang::{ValueUses, UntypedMembers, OwnMembers}`
  - Revisit if: a language's walk names the type of the values whose path it leaves empty, or the type path stops
    binding what the source reads
- [3.174 Mark a use as a type apart from a read](./adr/3.174-mark-a-use-as-a-type-apart-from-a-read.md)
  - Governs: `kartta_lang::Access::Type`, the walks of `kartta-lang-go` and `kartta-lang-python`, `Graph::references`
    (`SiteQuery` access)
  - Revisit if: a third language records the types its declarations name, and its sites of a type split otherwise
- [3.175 Answer the references of a field or a value by
  access](./adr/3.175-answer-the-references-of-a-field-or-a-value-by-access.md)
  - Governs: `Graph::references`, `SiteQuery`, `crates/kartta-graph/src/calls.rs`
  - Revisit if: agents ask for a field's sites by a distinction the four accesses do not draw
- [3.176 Bind no Rust call through a type outside the
  graph](./adr/3.176-bind-no-rust-call-through-a-type-outside-the-graph.md)
  - Governs: `kartta_lang::MemberCalls::ModulePaths`, `crates/kartta-graph/src/resolve.rs` (`resolve_by_name`,
    `through_the_receiver_s_owner`, `names_a_type_outside_the_graph`)
  - Revisit if: the Rust walk records the type a `use` brings in, renamed or not, and the prelude's names
- [4.1 Answer agents through an MCP server with the tools their prompts already
  name](./adr/4.1-answer-agents-through-an-mcp-server-with-the-tools-their-prompts-name.md)
  - Governs: `crates/kartta-mcp/src/{server,params,render,llmxml,config}.rs`, `crates/kartta-mcp/src/render/`
  - Revisit if: a consumer needs a transport other than stdio
- [4.2 Reach the engine through one JSON call over a small C
  ABI](./adr/4.2-reach-the-engine-through-one-json-call.md)
  - Governs: `crates/kartta-ffi`, `crates/kartta-ffi/include/kartta.h`, `crates/kartta-ffi/src/wire/`, `bindings/go`
  - Revisit if: a consumer's answers grow large enough that encoding them as JSON shows beside the query
- [4.3 Reach the engine from Node through an addon over the same
  dispatch](./adr/4.3-reach-the-engine-from-node-through-an-addon-over-the-same-dispatch.md)
  - Governs: `crates/kartta-node/src/lib.rs`, `bindings/node/src/{kartta,wire}.ts`
  - Revisit if: Node gains a stable, asynchronous FFI that can call the C ABI without an addon
- [4.4 Follow the indexed tree, and refresh it on
  request](./adr/4.4-follow-the-indexed-tree-and-refresh-it-on-request.md)
  - Governs: `crates/kartta/src/follow.rs` (`Follower`), `crates/kartta-mcp/src/{state,config}.rs`,
    `crates/kartta-mcp/src/render.rs` (`index`)
  - Revisit if: a tree raises more notifications while it is built than the system's queue holds, as a matter of
    course
- [4.10 Close a cut path list with a line no path can read
  as](./adr/4.10-close-a-cut-path-list-with-a-line-no-path-can-read-as.md)
  - Governs: `crates/kartta-mcp/src/llmxml.rs` (`line`, `List`), `crates/kartta-mcp/src/render/files.rs`
  - Revisit if: a consumer reads the path lists as data rather than as text
- [4.11 Group a symbol's references by file, the busiest
  first](./adr/4.11-group-a-symbol-s-references-by-file-the-busiest-first.md)
  - Governs: `crates/kartta-mcp/src/render/calls.rs` (`references`, `Listing`, `site_records`)
  - Revisit if: agents need the sites of a file past the 50 named often enough that the answer should name more
- [4.12 Count the calls of a symbol's name that several definitions leave
  unbound](./adr/4.12-count-the-calls-of-a-symbol-s-name-that-several-definitions-leave-unbound.md)
  - Governs: `crates/kartta-mcp/src/render/calls.rs` (`caller_match`, `no_callers`, `impact`, `seed`),
    `Walk::ambiguous`
  - Revisit if: an agent's next call depends on how many definitions split the ambiguous calls
- [4.13 Count what the confidence floor dropped in place of an empty answer's
  verdict](./adr/4.13-count-what-the-confidence-floor-dropped-in-place-of-an-empty-answer-s-verdict.md)
  - Governs: `crates/kartta-mcp/src/render/calls.rs` (`below_the_floor`, `no_callers`, `callee_match`,
    `reference_match`, `path_match`)
  - Revisit if: a walk records the band of each record it drops
- [4.14 List a symbol's call of itself apart from its callers and
  callees](./adr/4.14-list-a-symbol-s-call-of-itself-apart-from-its-callers-and-callees.md)
  - Governs: `crates/kartta-mcp/src/render/calls.rs` (`self_call`, `caller_match`, `callee_match`), `Walk::self_call`
  - Revisit if: the graph binds a call on a value to the type the value holds, so few self-edges are false
- [4.15 Name the definition that holds an unowned symbol in an
  outline](./adr/4.15-name-the-definition-that-holds-an-unowned-symbol-in-an-outline.md)
  - Governs: `crates/kartta-mcp/src/render/symbols.rs` (`outline`, `holder`, `holds`)
  - Revisit if: an extraction records holders, so the outline can read them in place of judging by lines
- [4.18 Answer a name that selects nothing with what it does
  match](./adr/4.18-answer-a-name-that-selects-nothing-with-what-it-does-match.md)
  - Governs: `crates/kartta/src/reading.rs` (`unmatched`, `selects_as_callers`), `crates/kartta-mcp/src/render.rs`
    (`unresolved`, `named_instead`, `did_you_mean`), `crates/kartta-mcp/src/server.rs` (`unmatched`, `suggestions`)
  - Revisit if: agents' misses are mostly other than a shared name, a prefix or a single edit
- [4.19 Name a language that records no use of a type in a type's
  references](./adr/4.19-name-a-language-that-records-no-use-of-a-type-in-a-type-s-references.md)
  - Governs: `crates/kartta-mcp/src/render/calls.rs` (`reference_match`, `no_sites`), `kartta_lang::TypeUses`
  - Revisit if: every shipped language records the uses of a type where code names it as one
- [4.20 Read a change from the new side of a unified
  diff](./adr/4.20-read-a-change-from-the-new-side-of-a-unified-diff.md)
  - Governs: `crates/kartta-mcp/src/diff.rs`, `crates/kartta-mcp/src/server.rs` (`changed_regions`)
  - Revisit if: agents hand changes in a form other than a unified diff often enough to pay for a second reader
- [4.21 Hold one repository's engine per process](./adr/4.21-hold-one-repository-s-engine-per-process.md)
  - Governs: `crates/kartta-mcp/src/state.rs` (`Slot`, `release`), `crates/kartta-mcp/src/server.rs` (`index_repo`,
    `index_at_launch`, `stop_on`), `crates/kartta-mcp/src/main.rs`
  - Revisit if: agents work across several repositories in one session often enough that rebuilds cost more than a
    second engine's memory
- [4.22 Name a language that records no read or write of a data kind in its
  references](./adr/4.22-name-a-language-that-records-no-read-or-write-of-a-data-kind-in-its-references.md)
  - Governs: `crates/kartta-mcp/src/render/calls.rs` (`reference_match`, `Unrecorded`, `no_sites`,
    `PARTIAL_DATA_ACCESS`), `kartta_lang::Conventions::unrecorded_data`, the `find_references` tool
  - Revisit if: every shipped language records the reads and writes of every data kind it extracts
- [4.23 Bound each MCP tool's answer by a default
  limit](./adr/4.23-bound-each-mcp-tool-s-answer-by-a-default-limit.md)
  - Governs: `crates/kartta-mcp/src/server.rs` (the limit constants), `crates/kartta-mcp/src/params.rs`
  - Revisit if: the prompts in use change the defaults they pass, or agents raise a tool's limit on most of its calls
- [4.24 Count in each MCP answer what it left out](./adr/4.24-count-in-each-mcp-answer-what-it-left-out.md)
  - Governs: `crates/kartta-mcp/src/render.rs`, `crates/kartta-mcp/src/render/`, `crates/kartta-mcp/src/llmxml.rs`
  - Revisit if: a count stops riding on the work its answer already does, so it costs each call
- [4.30 Cancel a build, a grep or a refresh by the id it names, even before it
  enters](./adr/4.30-cancel-a-build-a-grep-or-a-refresh-by-the-id-it-names-even-before-it-enters.md)
  - Governs: `crates/kartta-ffi/src/handle.rs` (`Calls`, `Kartta::cancel`), `crates/kartta-ffi/src/wire/build.rs`
    (`CancelParams`, `By`), `bindings/go/kartta/kartta.go` (`tracked`, `callTracked`, `Call`, `Close`)
  - Revisit if: a binding's close lands on more than 1,024 calls of one method still on their way to the library
- [4.31 Set no global allocator in the C ABI's archive](./adr/4.31-set-no-global-allocator-in-the-c-abi-s-archive.md)
  - Governs: `crates/kartta-ffi/Cargo.toml`, `crates/kartta-ffi/src/lib.rs`
  - Revisit if: build time matters more to a consumer than its peak, or a glibc build shows the musl time is the
    allocator's
- [4.32 Refresh the wire's engine in place where its build kept
  stamps](./adr/4.32-refresh-the-wire-s-engine-in-place-where-its-build-kept-stamps.md)
  - Governs: `crates/kartta-ffi/src/built.rs` (`Built::refresh`, `Shared`), `crates/kartta-ffi/src/handle.rs`
    (`Kartta::refresh`), `crates/kartta-ffi/src/wire/refresh.rs`, `bindings/go/wire/refresh.go`
  - Revisit if: a refresh without stamps comes to cost a fraction of a build, or queries cannot wait behind a refresh
- [4.33 Follow the tree over the wire only when a build
  asks](./adr/4.33-follow-the-tree-over-the-wire-only-when-a-build-asks.md)
  - Governs: `crates/kartta-ffi/src/built.rs` (`Shared::build_watched`, `Followed for Shared`),
    `crates/kartta-ffi/src/handle.rs` (`Kartta::build`, `Drop for Kartta`), `crates/kartta-ffi/src/wire/build.rs`
    (`WatchView`), `bindings/go/kartta/kartta.go` (cgo flags)
  - Revisit if: a wire consumer edits the trees it builds, so a watch would pay for itself by default
- [4.34 Say how a symbol method read a name, and why it matched
  nothing](./adr/4.34-say-how-a-symbol-method-read-a-name-and-why-it-matched-nothing.md)
  - Governs: `crates/kartta-ffi/src/query.rs` (`ask_named`, `call_path`), `crates/kartta-ffi/src/built.rs` (`Matches`),
    `crates/kartta/src/reading.rs` (`ask_reading`, `unmatched`, `selects_as_callers`), `bindings/go/wire/graph.go`
    (`Matches`, `Unmatched`, `GraphDefinition`)
  - Revisit if: consumers need alternatives for a name no case holds often enough that a search on every such miss pays
- [4.35 Flag a type's references where its language records no use as a
  type](./adr/4.35-flag-a-type-s-references-where-its-language-records-no-use-as-a-type.md)
  - Governs: `crates/kartta-ffi/src/wire/graph.rs` (`ReferencesView`), `bindings/go/wire/graph.go`
  - Revisit if: no shipped language states `TypeUses::Nowhere`
- [4.36 Pin each wire method in a golden file both bindings
  replay](./adr/4.36-pin-each-wire-method-in-a-golden-file-both-bindings-replay.md)
  - Governs: `crates/kartta-ffi/tests/golden/`, `crates/kartta-ffi/tests/fixture/`, `crates/kartta-ffi/tests/golden.rs`,
    `bindings/go/kartta/golden_test.go`
  - Revisit if: a score other than through `ln_1p` differs between platforms by more than a relative 1e-12
- [4.40 Ship the Node addon as one npm package per
  target](./adr/4.40-ship-the-node-addon-as-one-npm-package-per-target.md)
  - Governs: `bindings/node/npm/`, `bindings/node/src/target.ts`, `bindings/node/src/kartta.ts` (`load`),
    `bindings/node/package.json` (`engines`)
  - Revisit if: a consumer asks for Windows or Linux with musl
- [4.41 Check the Node package with the Vite+ that its lockfile
  pins](./adr/4.41-check-the-node-package-with-the-vite-plus-that-its-lockfile-pins.md)
  - Governs: `bindings/node/{package.json,package-lock.json,.nvmrc,vite.config.ts,tsconfig.json,scripts/build.ts}`,
    the `node` job of `.github/workflows/ci.yml`
  - Revisit if: the maintainers' other packages leave Vite+
- [4.42 Stop a Node call by a binding id its handle gives
  it](./adr/4.42-stop-a-node-call-by-a-binding-id-its-handle-gives-it.md)
  - Governs: `bindings/node/src/kartta.ts` (`Kartta.build`, `Kartta.refresh`, `Kartta.grep`, `Kartta.call`,
    `Kartta.close`)
  - Revisit if: a consumer keeps more than 1,024 calls of one kind waiting to enter the library at once
- [4.43 Run the follow in the engine's crate over a lock its owner
  lends](./adr/4.43-run-the-follow-in-the-engine-s-crate-over-a-lock-its-owner-lends.md)
  - Governs: `crates/kartta/src/follow.rs` (`Followed`, `Watch`), `crates/kartta/Cargo.toml` (`watch`),
    `crates/kartta-ffi/src/built.rs` (`Shared`), `crates/kartta-mcp/src/state.rs` (`Served`)
  - Revisit if: an owner needs to follow a tree under a lock it cannot lend for the length of a refresh
- [4.44 Refresh on request, and list what the tree fails
  now](./adr/4.44-refresh-on-request-and-list-what-the-tree-fails-now.md)
  - Governs: the `refresh` tool of `crates/kartta-mcp/src/server.rs`, `crates/kartta-mcp/src/render.rs` (`refresh`),
    `kartta::replace_failures`
  - Revisit if: notifications reach every place a followed tree lives, and a watch can no longer be off

## Deprecated

_None._

## Superseded

- [3.61 Name a Go anonymous struct by the names that lead to
  it](./adr/3.61-name-a-go-anonymous-struct-by-the-names-that-lead-to-it.md), by
  [3.170](./adr/3.170-own-a-go-anonymous-struct-s-fields-by-the-field-or-the-variable-it-types.md)
  - Governs: `crates/kartta-lang-go/src/walk/{anonymous,types}.rs`, `crates/kartta-graph/src/typed.rs`
  - Revisit if: the type tier takes a field or a variable as the owner of its anonymous struct's fields
