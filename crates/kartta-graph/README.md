# kartta-graph

A language-agnostic code graph: the definitions of one language family's files, found by name.

A `Graph` takes in one `kartta_lang::Extraction` per file, as the languages built on `kartta-lang` produce them, and
answers what a file defines and where a name is defined. It names no language: what differs between languages reaches it
as the extractions and the `Conventions` it is built with.

## A file at a time

`Graph::insert` replaces whatever the graph held for a path and `Graph::remove` forgets one. Files live in slots a
removal frees for the next insertion, and a definition is addressed by its file's slot and its place in the file, so a
graph follows a tree that changes without being built again, and one file going in or out renumbers nothing else.
Names are interned once per graph.

## Answers

- `Graph::outline` — what a file defines, ordered by line, an enclosing definition before those it holds; `None` for a
  file the graph does not hold, which an empty outline is not.
- `Graph::definitions` — where a bare or qualified name is defined, nearest to the asking file first, cut to a limit and
  counting what the limit left out.

Status: the API is new and is not published yet.
