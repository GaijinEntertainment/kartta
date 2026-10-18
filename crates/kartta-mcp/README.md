# kartta-mcp

An MCP server over the kartta engine: one repository's file index and code graph, answered to an agent as tools over
stdio.

```sh
kartta-mcp [ROOT]
```

`index_repo` builds the index of a repository root, and every other tool answers from it. A `ROOT` argument, or
`KARTTA_ROOT`, starts that build at launch; until it finishes, `index_stats` answers that it is building and every other
query answers an error saying so. One repository is indexed per process, and indexing another replaces it.

An answer is an XML-like envelope written for a model: one root element named for the question, counts as attributes,
one record per line, and an attribute such as `truncated="true"` only where it applies. The path searches answer plain
paths instead, one per line; a path holding a control character, or opening with a quote, is quoted and escaped as an
attribute value is. An answer that a cap, a depth limit or a filter cut says how much it left out.

| Variable | Effect |
| --- | --- |
| `KARTTA_ROOT` | the repository to index at launch, when no `ROOT` is given |
| `KARTTA_CACHE_DIR` | a directory builds share per-file data through |
| `KARTTA_GREP_INDEX` | `memory`, `file` or `mmap`: build a trigram index that narrows `grep` |
| `KARTTA_GREP_INDEX_DIR` | where a `file` or `mmap` trigram index lives; the temporary directory by default |
| `KARTTA_THREADS` | how many threads builds and queries run on; every core by default |

stdout carries protocol frames alone, and every diagnostic goes to stderr.
