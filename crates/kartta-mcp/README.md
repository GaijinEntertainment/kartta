# kartta-mcp

An MCP server over the kartta engine: one repository's file index and code graph, answered to an agent as tools over
stdio.

```sh
kartta-mcp [ROOT]
```

`index_repo` builds the index of a repository root, and every other tool answers from it. A `ROOT` argument, or
`KARTTA_ROOT`, starts that build at launch; until it finishes, `index_stats` answers that it is building and every other
query answers an error saying so. One repository is indexed per process, and indexing another replaces it.

A built index follows its tree through the operating system's change notifications, from a watch started before the
build walked the tree, and refreshes itself from each change, those made while it was built included. `index_stats` says
`watch="following"`, or `watch="off: <reason>"` when `KARTTA_WATCH` turned it off, the inotify watch limit is too low
for the tree's directories, the notifications stopped, or the platform has none. `refresh` brings the index up to the
tree at the paths it is given or across the whole tree, for a watch that is off or a change made where notifications
do not reach, such as a network mount or a container bind mount. Either reads again only what changed: every build
keeps the change time of each file it read. A query waits for a refresh under way.

An answer is an XML-like envelope written for a model: one root element named for the question, counts as attributes,
one record per line, and an attribute such as `truncated="true"` only where it applies. The path searches answer plain
paths instead, one per line; a path holding a control character, or opening with a quote, is quoted and escaped as an
attribute value is. An answer that a cap, a depth limit or a filter cut says how much it left out.

| Variable | Effect |
| --- | --- |
| `KARTTA_ROOT` | the repository to index at launch, when no `ROOT` is given |
| `KARTTA_CACHE_DIR` | a directory builds share per-file data through |
| `KARTTA_CACHE_ENTRY_TTL_SECS` | the age in seconds past which a cache entry expires; a week when empty or `0` |
| `KARTTA_GREP_INDEX` | `memory`, `file` or `mmap`: build a trigram index that narrows `grep` |
| `KARTTA_GREP_INDEX_DIR` | where a `file` or `mmap` trigram index lives; the temporary directory by default |
| `KARTTA_THREADS` | how many threads builds and queries run on; every core by default |
| `KARTTA_WATCH` | `off`, `0`, `false` or `no`: take in edits only through `refresh`; the tree is followed by default |

stdout carries protocol frames alone, and every diagnostic goes to stderr.
