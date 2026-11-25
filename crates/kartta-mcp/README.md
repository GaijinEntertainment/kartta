# kartta-mcp

An MCP server over the kartta engine: one repository's file index and code graph, answered to an agent as tools over
stdio.

```sh
kartta-mcp [ROOT]
```

`index_root` builds the index of a repository root, and every other tool answers from it. A `ROOT` argument, or
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
paths instead, one per line; a path holding a control character, or opening with a quote or a parenthesis, is quoted
and escaped as an attribute value is, and so is such a `path: error` record in the `<failures>` of `grep`, so no record
reads as the closing line in parentheses of a list that was cut. An answer that a cap, a depth limit or a filter cut
says how much it left out.

| Variable | Effect |
| --- | --- |
| `KARTTA_ROOT` | the repository to index at launch, when no `ROOT` is given |
| `KARTTA_CACHE_DIR` | a directory builds share per-file data through; `kartta` under the user's cache directory by default, none when empty |
| `KARTTA_CACHE_ENTRY_TTL_SECS` | the age in seconds past which a cache entry expires; a week when empty or `0` |
| `KARTTA_GREP_INDEX` | `memory`, `file` or `mmap`: where the trigram index that narrows `grep` lives; `mmap` by default, none when empty |
| `KARTTA_GREP_INDEX_DIR` | where a `file` or `mmap` trigram index lives; the temporary directory by default |
| `KARTTA_THREADS` | how many threads builds and queries run on; every core by default |
| `KARTTA_WATCH` | `off`, `0`, `false` or `no`: take in edits only through `refresh`; the tree is followed by default |

A server whose environment sets neither keeps a cache and a trigram index, because every session pays for its build and
an agent greps many times in one:

- **The cache** is `kartta` under `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows, and `$XDG_CACHE_HOME` or
  `~/.cache` elsewhere. Each build hashes the files it reads and keeps what it computed for them, shared by every
  checkout and worktree a server indexes. On a 171,628-file C++ checkout it held 590 MiB, and a build that found it
  warm took 5.8 s against 27-31 s without it, which the first build also took. On the 4.1 million files of a macOS data
  volume it held 8.1 GiB, and the first build took 770 s against 615 s without a cache or an index. An entry expires a
  week after the build that computed it and leaves once a later build rewrites its pack, so the cache's payloads stay
  within about twice what its unexpired entries take. `KARTTA_CACHE_DIR=` (empty) turns it off.
- **The trigram index** is a scratch file in the temporary directory, mapped and unlinked as it is created, so it
  leaves nothing behind once the server ends and adds no footprint. A build needs room there for about twice the index
  while it runs. On the checkout the index held 117 MiB, cost the build no measurable time, and
  answered a literal `grep` of the whole tree in 4 ms against 3.4 s without it. A tree full of text files that are not
  source pays for reading them: on the data volume a warm build took 276 s against 158 s, and the index held 3.7 GiB.
  A pattern with no three-byte literal reads every file with or without it. `KARTTA_GREP_INDEX=` (empty) turns it off.

stdout carries protocol frames alone, and every diagnostic goes to stderr.
