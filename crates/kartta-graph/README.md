# kartta-graph

A language-agnostic code graph: the definitions, calls and uses of one language family's files.

A `Graph` takes in one `kartta_lang::Extraction` per file, as the languages built on `kartta-lang` produce them, and
answers what a file defines, where a name is defined, and who calls what. It names no language: what differs between
languages reaches it as the extractions and the `Conventions` it is built with.

## A file at a time

`Graph::insert` replaces whatever the graph held for a path and `Graph::remove` forgets one. Files live in slots a
removal frees for the next insertion, and a definition is addressed by its file's slot and its place in the file, so a
graph follows a tree that changes without being built again, and one file going in or out renumbers nothing else.
Names are interned once per graph.

## Calls are resolved when a query asks

A file keeps its call sites and uses as its language wrote them, and a query resolves the sites of the name it asks
about. A call goes through a confidence-ranked cascade: the owner a receiver names, the caller's own scope, its
imports, its package, its includes and the re-export chains through them, the caller's own owner, the one callable of
the name, and the callables nearest the caller. A member call, `x.f()`, binds a free function only where its language's
conventions let one stand for what a value holds, and never a free variable or constant, whose name no property of a
value reads. A use outside a call binds more narrowly: through the import its qualifier names, else a package-level
definition of its own file, of a dot import, or of its package directory. Each tier's confidence is a constant of
`kartta_graph::confidence`. No edge is stored, so nothing a file holds goes stale when another file changes.

## Answers

- `Graph::outline` — what a file defines, ordered by line, an enclosing definition before those it holds; `None` for a
  file the graph does not hold, which an empty outline is not.
- `Graph::definitions` — where a bare or qualified name is defined, nearest to the asking file first, cut to a limit and
  counting what the limit left out.
- `Graph::search` — the definitions whose names and file paths share the most words with a query, ranked by BM25
  weighed by the share of the query's words each holds and doubled for those in query order, a name that is the query
  first, generated and vendored code demoted, counting every match, those a name matched rather than a path, the
  query's words no definition holds and those past the eighth. When the words match fewer definitions than the limit,
  a fill-up adds those whose name or qualified name spells the query in order, and counts them. Each file keeps the
  distinct words of its definitions and path, gathered by the first search, so building spends nothing on search and a
  search splits only the names of the files that may hold a word it asks for.
- `Graph::callers` and `Graph::callees` — the callers or callees of a callable, to a depth, above a confidence floor and
  under a symbol cap, counting what the cap, the depth bound and the floor left out.
- `Graph::references` — the sites that call or use a symbol, by file and line, counting what the floor and the limit
  left out.
- `Graph::call_path` — the shortest chain of calls from one callable to another, counting what the cap, the depth bound
  and the floor left out.
- `Graph::dependents` — the files that include a file, transitively, nearest first, to a depth and under a file cap,
  counting the includers the cap turned away and the files reached whose own includers the walk did not read; `None`
  for a file the graph does not hold. The includers of each file are kept as an index, updated as files go in and out.
- `Graph::implementations` — the types that name a type as a parent, transitively, to a depth and cut to a limit,
  counting the implementors the limit left out and the types reached whose own implementors the query did not look
  for. A written parent names the type its language's scopes reach: a type of its package directory or of the
  directory its import binds where a package is a directory, the type its owner chain or its enclosing scopes reach
  first where a language has them, else the type of that name nearest the file.
- `Graph::analyze_segment` — the symbols a range of a file's lines overlaps, each with its direct callers and callees,
  at most 50 of each listed and all of them counted, and the blast of a change to it: the callers a walk over its
  callers reaches by depth, the files they and the file's includers sit in, and a reach band, counting what the cap,
  the depth bound and the floor left out; `None` for a file the graph does not hold. Each name's call sites are
  resolved once for the whole segment.
- `Graph::analyze_change` — what a change to a set of line regions reaches: the symbols they overlap, a declaration
  and its definition counting once, the callers a walk over each one's callers reaches, the footprint those walks
  cover together, and the hotspots two or more of them reach, at most 300 hotspots and 300 changed symbols listed and
  all of them counted. Each changed symbol lists its direct callers while 30 or fewer pass the floor, and counts them
  by directory past that. The regions whose file the graph does not hold, those that overlap no symbol, and the walks
  the cap stopped are counted. `Change::merge` ranks the answers of several graphs together, as a change across
  language families needs.
- `Graph::dead_code` — the definitions no call and no use reaches, in path order, cut to a limit and counting every one
  found, those found reached, and those it did not judge, by reason: of a kind not asked for; test, generated, entry
  and vendored code; a kind no call and no use shows used; a definition a call that binds nothing, or binds only by
  proximity, may reach; and a method a call through another type may reach. Every call and use that spells a judged
  name is resolved, so a query over a whole graph costs what resolving its calls costs.
- `Graph::stats` — what the graph holds: its files and those with no definition, its definitions by kind, the calls,
  local calls and uses inside definitions, and the includes, counted when asked.
- `Graph::resolution` — how every call site binds: the sites bound at each confidence, the caller and callee pairs they
  make, and the sites that bound nothing by reason — an import of code outside the repository, an import that exports
  no callable of the name, a name no definition has, definitions the call cannot bind, and a choice nothing settles —
  listing the first of them in path order up to a limit and counting the rest. It resolves every site, in parallel.

Status: the API is new and is not published yet.
