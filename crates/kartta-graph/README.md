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
the name, and the callables nearest the caller. A use outside a call binds more narrowly: through the import its
qualifier names, else a package-level definition of its own file, of a dot import, or of its package directory. Each
tier's confidence is a constant of `kartta_graph::confidence`. No edge is stored, so nothing a file holds goes stale
when another file changes.

## Answers

- `Graph::outline` — what a file defines, ordered by line, an enclosing definition before those it holds; `None` for a
  file the graph does not hold, which an empty outline is not.
- `Graph::definitions` — where a bare or qualified name is defined, nearest to the asking file first, cut to a limit and
  counting what the limit left out.
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

Status: the API is new and is not published yet.
