# ADR Index

## Accepted

- [1.1 Own the filesystem walk](./adr/1.1-own-the-filesystem-walk.md) — initiative `01-kartta-fs-port`, 2026-08-11,
  revised 2026-10-14
- [1.2 Own the path scorer](./adr/1.2-own-the-path-scorer.md) — initiative `01-kartta-fs-port`, 2026-08-11, revised
  2026-10-18
- [1.3 Make the ignore source set explicit](./adr/1.3-make-the-ignore-source-set-explicit.md) — initiative
  `01-kartta-fs-port`, 2026-08-24
- [1.4 Take the query glob dialect from globset](./adr/1.4-take-the-query-glob-dialect-from-globset.md) — initiative
  `01-kartta-fs-port`, 2026-08-24, revised 2026-10-13
- [1.5 Announce a cap unless the order already explains
  it](./adr/1.5-announce-a-cap-unless-the-order-explains-it.md) — initiative `01-kartta-fs-port`, 2026-08-24, revised
  2026-10-14
- [1.6 Read a file's own facts on demand](./adr/1.6-read-a-file-s-own-facts-on-demand.md) — initiative
  `01-kartta-fs-port`, 2026-08-24, revised 2026-10-14
- [1.7 Let a caller compose the binary extension set](./adr/1.7-let-a-caller-compose-the-binary-extension-set.md) —
  initiative `01-kartta-fs-port`, 2026-10-13
- [1.8 Count what a listing leaves out](./adr/1.8-count-what-a-listing-leaves-out.md) — initiative `01-kartta-fs-port`,
  2026-10-13
- [1.9 Front-code the stored paths](./adr/1.9-front-code-the-stored-paths.md) — initiative `01-kartta-fs-port`,
  2026-10-14, revised 2026-10-15
- [1.10 Search content through ripgrep's engine](./adr/1.10-search-content-through-ripgrep-s-engine.md) — initiative
  `01-kartta-fs-port`, 2026-10-14
- [1.11 Say what a limited content search did not
  cover](./adr/1.11-say-what-a-limited-content-search-did-not-cover.md) — initiative `01-kartta-fs-port`, 2026-10-14,
  revised 2026-10-14
- [1.12 Read each wanted file once, during the walk](./adr/1.12-read-each-wanted-file-once-during-the-walk.md) —
  initiative `01-kartta-fs-port`, 2026-10-14, revised 2026-10-14
- [1.13 Let the caller bound the index's threads](./adr/1.13-let-the-caller-bound-the-index-s-threads.md) — initiative
  `01-kartta-fs-port`, 2026-10-14
- [1.14 Narrow content search with a trigram index built from the walk's
  reads](./adr/1.14-narrow-content-search-with-a-trigram-index.md) — initiative `01-kartta-fs-port`, 2026-10-14,
  revised 2026-10-15
- [1.15 Refresh an index in place from a second walk](./adr/1.15-refresh-an-index-in-place-from-a-second-walk.md) —
  initiative `01-kartta-fs-port`, 2026-10-15, revised 2026-10-15
- [1.16 Refresh the parts of a tree a caller names](./adr/1.16-refresh-the-parts-of-a-tree-a-caller-names.md) —
  initiative `01-kartta-fs-port`, 2026-10-15, revised 2026-10-15
- [1.17 Follow a tree through its change notifications](./adr/1.17-follow-a-tree-through-its-change-notifications.md) —
  initiative `01-kartta-fs-port`, 2026-10-15, revised 2026-10-18
- [1.18 Return the memory a build freed to the system](./adr/1.18-return-the-memory-a-build-freed-to-the-system.md) —
  initiative `01-kartta-fs-port`, 2026-10-17, revised 2026-10-18
- [1.19 Prune editor and OS clutter by name during the
  walk](./adr/1.19-prune-editor-and-os-clutter-by-name-during-the-walk.md) — initiative `01-kartta-fs-port`, 2026-10-17
- [2.1 Share per-file data through content-addressed
  packs](./adr/2.1-share-per-file-data-through-content-addressed-packs.md) — initiative `02-shared-cache`, 2026-10-16,
  revised 2026-10-18
- [2.2 Take per-file data from the shared cache in the walk's
  read](./adr/2.2-take-per-file-data-from-the-cache-in-the-walk-s-read.md) — initiative `02-shared-cache`, 2026-10-16
- [2.3 Expire cache entries by the time each was
  published](./adr/2.3-expire-cache-entries-by-the-time-each-was-published.md) — initiative `02-shared-cache`,
  2026-10-18
- [3.1 Plug languages in through a trait](./adr/3.1-plug-languages-in-through-a-trait.md) — initiative `03-code-graph`,
  2026-10-16
- [3.2 Name a Go call's target only where the syntax
  spells it](./adr/3.2-name-a-go-call-s-target-only-where-the-syntax-spells-it.md) — initiative `03-code-graph`,
  2026-10-16
- [3.3 Repair what the C++ grammar cannot parse instead of vendoring
  it](./adr/3.3-repair-what-the-c-plus-plus-grammar-cannot-parse.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-17
- [3.4 Extract TypeScript and JavaScript as one
  family](./adr/3.4-extract-typescript-and-javascript-as-one-family.md) — initiative `03-code-graph`, 2026-10-16
- [3.5 Keep the graph a file at a time](./adr/3.5-keep-the-graph-a-file-at-a-time.md) — initiative `03-code-graph`,
  2026-10-16, revised 2026-10-17
- [3.6 Build the graph inside the index's walk](./adr/3.6-build-the-graph-inside-the-index-s-walk.md) — initiative
  `03-code-graph`, 2026-10-16
- [3.7 Resolve calls when a query asks](./adr/3.7-resolve-calls-when-a-query-asks.md) — initiative `03-code-graph`,
  2026-10-16, revised 2026-10-17
- [3.8 Extract only the daslang a file opts into gen2, and find modules by file
  name](./adr/3.8-extract-only-the-daslang-a-file-opts-into-gen2.md) — initiative `03-code-graph`, 2026-10-16
- [3.9 Resolve a Quirrel import through the nearest .sqconfig, and name a function value by its
  binding](./adr/3.9-resolve-a-quirrel-import-through-the-nearest-sqconfig.md) — initiative `03-code-graph`, 2026-10-16,
  revised 2026-10-17
- [3.10 Resolve a BLK include as the engine loads it, and read the mount table from the files that state it by
  name](./adr/3.10-resolve-a-blk-include-as-the-engine-loads-it.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-17
- [3.11 Name Rust items by their module chain and resolve modules by Cargo's
  layout](./adr/3.11-name-rust-items-by-their-module-chain-and-resolve-modules-by-cargo-s-layout.md) — initiative
  `03-code-graph`, 2026-10-16
- [3.12 Qualify Python by the module its path
  names](./adr/3.12-qualify-python-by-the-module-its-path-names.md) — initiative `03-code-graph`, 2026-10-16, revised
  2026-10-18
- [3.13 Read a shell script's commands as calls and its sources as
  includes](./adr/3.13-read-a-shell-script-s-commands-as-calls-and-its-sources-as-includes.md) — initiative
  `03-code-graph`, 2026-10-16
- [3.14 Fold a PHP include path and map a use through
  PSR-4](./adr/3.14-fold-a-php-include-path-and-map-a-use-through-psr-4.md) — initiative `03-code-graph`, 2026-10-16
- [3.15 Qualify a Ruby member by its whole namespace
  chain](./adr/3.15-qualify-a-ruby-member-by-its-whole-namespace-chain.md) — initiative `03-code-graph`, 2026-10-16
- [3.16 Load a Lua module only from a literal name](./adr/3.16-load-a-lua-module-only-from-a-literal-name.md) —
  initiative `03-code-graph`, 2026-10-16
- [3.17 Resolve a Puppet class through the autoloader's
  layout](./adr/3.17-resolve-a-puppet-class-through-the-autoloader-s-layout.md) — initiative `03-code-graph`, 2026-10-16
- [3.18 Extract DSHL as one namespace that its includes
  join](./adr/3.18-extract-dshl-as-one-namespace-its-includes-join.md) — initiative `03-code-graph`, 2026-10-16
- [3.19 Find the files that include a file through an index of
  includers](./adr/3.19-find-the-files-that-include-a-file-through-an-index-of-includers.md) — initiative
  `03-code-graph`, 2026-10-17
- [3.20 Resolve implementors from the parents written when a query
  asks](./adr/3.20-resolve-implementors-from-the-parents-written-when-a-query-asks.md) — initiative `03-code-graph`,
  2026-10-17
- [3.21 Search definitions by the words of their names and
  paths](./adr/3.21-search-definitions-by-the-words-of-their-names-and-paths.md) — initiative `03-code-graph`,
  2026-10-17
- [3.22 Analyze what a change reaches with the walk the call queries
  take](./adr/3.22-analyze-what-a-change-reaches-with-the-walk-the-call-queries-take.md) — initiative `03-code-graph`,
  2026-10-17
- [3.23 Judge code dead only where the graph sees it
  used](./adr/3.23-judge-code-dead-only-where-the-graph-sees-it-used.md) — initiative `03-code-graph`, 2026-10-17,
  revised 2026-10-18
- [3.24 Count what a graph holds and how its calls bind when
  asked](./adr/3.24-count-what-a-graph-holds-and-how-its-calls-bind-when-asked.md) — initiative `03-code-graph`,
  2026-10-17
- [3.25 Read a definition's text and doc comment from its file when
  asked](./adr/3.25-read-a-definition-s-text-and-doc-comment-from-its-file-when-asked.md) — initiative
  `03-code-graph`, 2026-10-17
- [3.29 Name a Go receiver by the type its declarations
  spell](./adr/3.29-name-a-go-receiver-by-the-type-its-declarations-spell.md) — initiative `03-code-graph`, 2026-10-17
- [3.30 Look a repository path up without ASCII case through a table built when first
  asked](./adr/3.30-look-a-repository-path-up-without-case-through-a-table-built-when-first-asked.md) — initiative
  `03-code-graph`, 2026-10-17
- [3.31 Parse a C or C++ table without its numbers, and hold one tree at a
  time](./adr/3.31-parse-a-c-or-c-plus-plus-table-without-its-numbers.md) — initiative `03-code-graph`, 2026-10-18
- [3.32 Hand an engine every shipped language from one crate, and select them by
  name](./adr/3.32-hand-an-engine-every-shipped-language-from-one-crate.md) — initiative `03-code-graph`, 2026-10-18
- [3.33 Follow a tree through the questions each file's resolution
  asked](./adr/3.33-follow-a-tree-through-the-questions-each-file-s-resolution-asked.md) — initiative `03-code-graph`,
  2026-10-18
- [3.34 Record each read and write of a field or a value, and bind it when a query
  asks](./adr/3.34-record-each-read-and-write-of-a-field-or-a-value.md) — initiative `03-code-graph`, 2026-10-18,
  revised 2026-10-18
- [4.1 Answer agents through an MCP server with the tools their prompts already
  name](./adr/4.1-answer-agents-through-an-mcp-server-with-the-tools-their-prompts-name.md) — initiative
  `04-consumer-surfaces`, 2026-10-18
- [4.2 Reach the engine through one JSON call over a small C
  ABI](./adr/4.2-reach-the-engine-through-one-json-call.md) — initiative `04-consumer-surfaces`, 2026-10-18
- [4.3 Reach the engine from Node through an addon over the same
  dispatch](./adr/4.3-reach-the-engine-from-node-through-an-addon-over-the-same-dispatch.md) — initiative
  `04-consumer-surfaces`, 2026-10-18
- [4.4 Follow the indexed tree, and refresh it on
  request](./adr/4.4-follow-the-indexed-tree-and-refresh-it-on-request.md) — initiative `04-consumer-surfaces`,
  2026-10-18

## Deprecated

_None._

## Superseded

_None._
