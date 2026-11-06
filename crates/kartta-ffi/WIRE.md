# The kartta wire

The reference of how to link the library and of what `kartta_call` takes and answers: every method, its params, its
answer, every default, error code and enumerated value. [`include/kartta.h`](include/kartta.h) states the C functions,
who owns what, and the rules on threads and panics; this file assumes them. `tests/reference.rs` holds this file to the
code, so a name the code adds fails the crate's tests until a section here names it.

Each `###` section below is one JSON object or one set of values. Its heading is this reference's label, which never
appears on the wire; the names on the wire are the bulleted ones. A field reads `name (type, presence)`: a param is
`required` or `optional`, and an answer's field is `always` there or `optional`, absent where it would say nothing.

## Linking

Run from the directory of a release's `kartta-ffi` tarball, on the target it names, these commands link `main.c`.
Every release build runs each row on a small C program (`scripts/link-check.sh`). `rustc --print native-static-libs`
is no substitute: its `-lunwind` fails on Alpine, where gcc brings the unwinder.

| Target | Command |
| ---------------------------- | ------------------------------------------------------------------------------------ |
| `aarch64-apple-darwin` | `cc main.c -L. -lkartta_ffi -framework CoreServices -framework CoreFoundation -liconv` |
| `aarch64-unknown-linux-gnu` | `cc main.c -L. -lkartta_ffi -lpthread -lm -ldl` |
| `x86_64-unknown-linux-gnu` | `cc main.c -L. -lkartta_ffi -lpthread -lm -ldl` |
| `aarch64-unknown-linux-musl` | `cc main.c -L. -lkartta_ffi` |
| `aarch64-unknown-linux-musl` | `cc -static main.c -L. -lkartta_ffi` |
| `x86_64-unknown-linux-musl` | `cc main.c -L. -lkartta_ffi` |
| `x86_64-unknown-linux-musl` | `cc -static main.c -L. -lkartta_ffi` |
| `x86_64-pc-windows-msvc` | `cl -MD main.c kartta_ffi.lib kernel32.lib ntdll.lib userenv.lib ws2_32.lib dbghelp.lib` |
| `x86_64-pc-windows-gnu` | `gcc main.c -L. -lkartta_ffi -lntdll -luserenv -lws2_32 -ldbghelp` |

## Calling

- `kartta_open` takes [`OpenParams`](#openparams). It reads the root's metadata and walks nothing.
- `kartta_call` takes a method name and a JSON object of params, and answers [`Answered`](#answered) or
  [`Failed`](#failed). Branch on the `code` of an [`Error`](#error); its `message` is for a reader, and may change.
- Null params with a zero length, empty text, text of whitespace alone and the JSON `null` all read as `{}`.
- A method refuses a field it does not know with `invalid_params`, and any JSON that is neither an object nor `null`.
- An answer may gain a field in any release. A consumer ignores the keys it does not know.
- An integer fits an unsigned 64-bit value; a param a line or a depth fits 32 bits, `min_confidence` 8 bits, and `score`
  of a [`FuzzyMatchView`](#fuzzymatchview) is a signed 32-bit value.
- A path is relative to the root and uses `/`, except the `path` of a [`StoredError`](#storederror), which is the path
  as the system spells it, and `content_cache` and the `dir` of a [`ContentIndex`](#contentindex), which reach the
  system as the caller wrote them.
- A bound left out takes the default of [Bounds](#bounds) its field names. A bound of 0 means what its field says.
- A `min_confidence` left out takes 0, which turns no call or site away.
- A count left out of an answer reads as 0, except `dropped`, `fuzzy_total`, `matched`, `total_files` and `total_lines`,
  whose fields say when they are counted: there, absent means not counted.
- A count holds what the graph records, so 0 does not show that nothing uses a symbol: a language records only some
  uses, and `type_uses_unrecorded` and `data_uses_unrecorded` name where it records none of a kind.
- `version` needs no build. Every other method but `build` and `cancel` asks what the last finished build made, and
  answers `not_built` or `building` when there is none. `fuzzy`, `glob`, `grep`, `list` and `stats` also answer from the
  index a build a cancel stopped left, and name it in `stopped_build`, [`StoppedView`](#stoppedview).
- `build`, `grep` and `refresh` take an `id` of the caller's choosing, 0 among them, which `cancel` stops them by. A
  binding over the wire gives each call it runs a `binding_id` as well, so that its own cancels never meet its callers'
  ids. `cancel` with neither stops every call in flight. Each of these methods blocks its calling thread until it
  answers.

## Surface

### Methods

- `analyze_change`: params [`ChangeParams`](#changeparams), answer [`ChangeView`](#changeview). What a change to regions
  across files reaches. It asks every graph.
- `analyze_segment`: params [`SegmentParams`](#segmentparams), answer [`FileView`](#fileview) of
  [`Segment`](#segment). What a change to a range of a file's lines reaches.
- `build`: params [`BuildParams`](#buildparams), answer [`StatsAnswer`](#statsanswer). Walks the tree once and builds
  the file index and every graph. Builds run one at a time, and each drops the engine the handle held before it walks. A
  build that fails leaves the handle with no engine. A build a cancel stops leaves the index of what it reached, unless
  it reached nothing, and no engine. A build that finished before its cancel landed answers `build_cancelled` and leaves
  its engine.
- `call_path`: params [`CallPathParams`](#callpathparams), answer [`Matches`](#matches) of [`CallPath`](#callpath). A
  chain of calls between two symbols.
- `callees`: params [`WalkParams`](#walkparams), answer [`Matches`](#matches) of [`Walk`](#walk). What a symbol calls,
  `CALLEES_DEPTH` calls deep by default.
- `callers`: params [`WalkParams`](#walkparams), answer [`Matches`](#matches) of [`CallersView`](#callersview). What
  calls a symbol, `CALLERS_DEPTH` calls deep by default.
- `cancel`: params [`CancelParams`](#cancelparams), answer [`CancelAnswer`](#cancelanswer). Stops a build, a grep or a
  refresh, and never a watch or any other query.
- `dead_code`: params [`DeadCodeParams`](#deadcodeparams), answer [`Matches`](#matches) of [`DeadCode`](#deadcode). The
  definitions no call and no use the graph holds reaches, one part per graph.
- `definitions`: params [`DefinitionsParams`](#definitionsparams), answer [`Matches`](#matches) of
  [`DefinitionsView`](#definitionsview). Where a name is defined.
- `dependents`: params [`DependentsParams`](#dependentsparams), answer [`FileView`](#fileview) of
  [`Dependents`](#dependents). The files that include a file, directly or through others.
- `fuzzy`: params [`FuzzyParams`](#fuzzyparams), answer [`FuzzyView`](#fuzzyview). The paths a query ranks best.
- `glob`: params [`GlobParams`](#globparams), answer [`GlobView`](#globview). The paths that match globs, in index
  order.
- `grep`: params [`GrepParams`](#grepparams), answer [`GrepView`](#grepview). Searches file content.
- `implementations`: params [`ImplementationsParams`](#implementationsparams), answer [`Matches`](#matches) of
  [`Implementations`](#implementations). The types that implement or extend a type.
- `list`: params [`ListParams`](#listparams), answer [`ListView`](#listview). A directory's entries.
- `outline`: params [`OutlineParams`](#outlineparams), answer [`FileView`](#fileview) of
  [`OutlineView`](#outlineview). The definitions of one file.
- `references`: params [`ReferencesParams`](#referencesparams), answer [`Matches`](#matches) of
  [`ReferencesView`](#referencesview). The sites that call or use a symbol.
- `refresh`: params [`RefreshParams`](#refreshparams), answer [`RefreshAnswer`](#refreshanswer). Brings the engine up to
  the tree in place; a query waits for it to finish.
- `resolution`: params [`ResolutionParams`](#resolutionparams), answer [`Matches`](#matches) of
  [`Resolution`](#resolution). How the call sites of each graph bound.
- `search`: params [`SearchParams`](#searchparams), answer [`Matches`](#matches) of [`Search`](#search). Definitions by
  the words of their names and paths, one part per graph with a hit, or with a definition `total` counts.
- `sources`: params [`SourcesParams`](#sourcesparams), answer [`Matches`](#matches) of
  [`DefinitionsView`](#definitionsview). The text of a name's definitions.
- `stats`: params [`NoParams`](#noparams), answer [`StatsAnswer`](#statsanswer). What the last build made, as the
  index holds it now.
- `version`: params [`NoParams`](#noparams), answer [`About`](#about). The library's versions, methods and languages.

### Languages

The names `languages` of [`BuildParams`](#buildparams) takes. `lang` in the params of a graph query takes a language or
its family, and a family names one graph.

- `bash` (family `bash`)
- `blk` (family `blk`)
- `cpp` (family `cpp`)
- `daslang` (family `daslang`)
- `dshl` (family `dshl`)
- `go` (family `go`)
- `javascript` (family `jsts`)
- `lua` (family `lua`)
- `php` (family `php`)
- `puppet` (family `puppet`)
- `python` (family `python`)
- `quirrel` (family `quirrel`)
- `ruby` (family `ruby`)
- `rust` (family `rust`)
- `tsx` (family `jsts`)
- `typescript` (family `jsts`)

### Bounds

- `ABI_VERSION`: 1. What `kartta_abi_version` returns, and `abi` of [`About`](#about).
- `CALLEES_DEPTH`: 1. How many calls deep `callees` goes.
- `CALLERS_DEPTH`: 3. How many calls deep `callers` goes.
- `CALL_PATH_DEPTH`: 8. How many calls long a path of `call_path` may be.
- `CALL_PATH_MAX_SYMBOLS`: 20,000. The symbols the search of `call_path` reaches.
- `CANCELLED_AHEAD_LIMIT`: 1,024. The named cancels that stopped nothing a handle remembers, per kind of call and per
  name, for a call that has not entered yet; a further one forgets the oldest.
- `DEAD_CODE`: 100. The definitions `dead_code` answers.
- `DEFINITIONS`: 10. The definitions `definitions` and `sources` answer.
- `DEPENDENTS`: 100. The files `dependents` answers.
- `DEPENDENTS_DEPTH`: 3. How many includes away `dependents` goes.
- `ERRORS_LISTED`: 20. The failures a [`Listed`](#listed) lists.
- `FUZZY_LIMIT`: 50. The matches `fuzzy` answers.
- `GREP_LIMIT`: 100. The matching lines `grep` answers.
- `GREP_MAX_FILE_BYTES`: 4,194,304. The size past which `grep` does not search a file.
- `GREP_MAX_LINE_BYTES`: 1,024. The bytes of each line `grep` answers.
- `IMPLEMENTATIONS`: 100. The implementors `implementations` answers.
- `IMPLEMENTATIONS_DEPTH`: 3. How many levels below the root `implementations` goes.
- `LIST_DEPTH`: 1. How many levels `list` descends.
- `LIST_LIMIT`: 1,000. The entries `list` answers.
- `REFERENCES`: 500. The sites `references` answers.
- `SEARCH`: 20. The hits `search` answers.
- `SEGMENT_DEPTH`: 3. How many calls deep each walk of `analyze_segment` and `analyze_change` goes.
- `UNBOUND`: 20. The unbound call sites `resolution` lists per graph.
- `UNMATCHED_LISTED`: 5. The definitions an [`Unmatched`](#unmatched) lists.
- `WALK_MAX_SYMBOLS`: 500. The symbols a walk reaches, its root included.

## The envelope

### Answered

- `ok` (the method's answer, always): The answer the method's row in [Methods](#methods) names.

### Failed

- `err` ([`Error`](#error), always): Why the call failed.

### Error

Also what `kartta_open` writes to its error out-parameter.

- `code` ([`ErrorCode`](#errorcode), always): What to branch on.
- `message` (string, always): What failed and, where the handle knows it, what to ask instead.

### ErrorCode

- `build_cancelled`: A cancel stopped the build, whose index the file queries answer from when it holds an entry, or
  that the cancel came too late to stop.
- `build_failed`: The build stopped on a failure, which leaves no index.
- `building`: A build is in flight; ask again once it finishes.
- `internal`: The engine panicked, `kartta_call` could not start its thread, or an answer could not be encoded. The
  handle stays usable, though a refresh that panicked partway leaves every call that reads the engine answering this
  code, naming `build`, until a build replaces the engine. A `kartta_open` that answers it made no handle.
- `invalid_glob`: A glob the params name does not compile, answered by the call that takes it.
- `invalid_params`: Params the method does not take, the message naming what: JSON that is neither an object nor `null`,
  a field that is unknown, missing or of the wrong type, a value the method refuses, or a graph the build did not make.
- `invalid_pattern`: A grep pattern does not compile.
- `invalid_request`: A null handle or method, a method or params that are not UTF-8, null params with a nonzero length,
  or params that are not JSON.
- `invalid_root`: `kartta_open` could not read the root's metadata, or the root is not a directory.
- `no_stamps`: The build kept no stamps, so a refresh is refused.
- `not_built`: No build has finished, or the last one failed, or a cancel stopped it and the method needs more than its
  index; the message then says what still answers.
- `refresh_cancelled`: A cancel stopped the refresh; the engine answers as it did before it.
- `refresh_failed`: The refresh stopped on a failure; the engine answers as it did before it.
- `unknown_method`: No method of the name; the message lists the methods.

### ListErrorCode

The codes of `list`, beside those of [`ErrorCode`](#errorcode).

- `not_a_directory`: The path is not a directory.
- `not_found`: The index does not hold the path.

## Params

### OpenParams

- `root` (string, required): The tree's root.

### NoParams

The params of a method that takes none: `{}`, or any form that reads as it.

### BuildParams

Left empty, a build indexes every language kartta ships, under the tree's own `.gitignore` files, with no content index
and no cache.

- `id` (integer, optional): Names the build for a cancel.
- `binding_id` (integer, optional): Names the build for a cancel `by` `binding_id`, apart from `id`.
- `languages` (array of string, optional): The languages to build graphs of, by the names of
  [Languages](#languages). Absent builds every one, empty none.
- `threads` (integer, optional): Bounds the threads the index runs on, at least 1. Absent takes every core, and 0
  answers `invalid_params`.
- `ignore` ([`Ignore`](#ignore), optional): Which ignore rules apply, `in_tree` when absent.
- `pruned_names` ([`SetChange`](#setchange), optional): Changes the entry names the walk neither indexes nor descends,
  editor and OS clutter by default.
- `binary_extensions` ([`SetChange`](#setchange), optional): Changes the extensions that settle a file as binary unread.
- `content_index` ([`ContentIndex`](#contentindex), optional): Builds a trigram index of file content, kept where it
  says. Absent builds none.
- `content_cache` (string, optional): A directory per-file data is shared through across builds.
- `content_cache_ttl_secs` (integer, optional): Seconds after the build that computed it began that a cache entry
  expires. Absent or 0 is a week.
- `stamps` (boolean, optional): Keeps the time each file the build reads last changed, so that a refresh reads again
  only the files that changed since and those a change reaches. It holds memory for as long as the index lives. Absent
  is false, unless `watch` is set, which keeps stamps and answers `invalid_params` for an explicit false.
- `watch` (boolean, optional): Follows the tree through the operating system's change notifications: a thread of the
  library's own refreshes the index from each change until the next build or `kartta_free`. False when absent.

### Ignore

- `in_tree` (default): Applies the `.gitignore` files inside the tree.
- `none`: Applies no ignore rule.

### SetChange

An object with exactly one of these keys.

- `extend`: An array of names to add to the built-in set.
- `replace`: An array of names to use in place of the built-in set.

### ContentIndex

An object whose `storage` key names where the index lives.

- `storage`: `memory`, `file` or `mapped`.
- `memory`: Holds the index in memory, and refuses `dir`.
- `file`: Holds the index in scratch files in `dir`.
- `mapped`: Holds the index in scratch files in `dir`, mapped into memory.
- `dir`: The directory the `file` and `mapped` storages create their scratch files in, which both require.

### CancelParams

Naming no call stops every build in flight, running or waiting its turn, every grep and every refresh in flight. A call
that enters afterwards runs: to stop a call that may not have entered, name it.

- `build` (integer, optional): The id of the build to stop.
- `grep` (integer, optional): The id of the grep to stop.
- `refresh` (integer, optional): The id of the refresh to stop.
- `by` ([`By`](#by), optional): The name of a call the ids above are compared with, `id` when absent.

### By

An id by one name never stops a call by the other.

- `id` (default): The `id` the call's caller gave it.
- `binding_id`: The `binding_id` a binding gave it.

### RefreshParams

Left empty, a refresh walks the whole tree again.

- `id` (integer, optional): Names the refresh for a cancel.
- `binding_id` (integer, optional): Names the refresh for a cancel `by` `binding_id`, apart from `id`.
- `paths` (array of string or null, optional): The root-relative paths that changed, spelled as the index's answers
  spell them. Absent or null refreshes the whole tree, and empty walks nothing. A path that starts with `/` or `\`,
  starts with a drive such as `C:`, or holds a `\` or a `..` name answers `invalid_params`, the message naming which,
  before `not_built` or `no_stamps` would.

### WalkParams

- `name` (string, required): Selects the symbol to start from, a bare or a qualified name.
- `path_prefix` (string, optional): Keeps only the symbols in files whose root-relative path starts with it, one file's
  path or a directory's with its closing slash. Absent looks in every file.
- `line` (integer, optional): Keeps only the symbols that start on this one-based line. Absent looks at every line, and
  0 selects none.
- `lang` (string, optional): Names the graph to ask, by a language or its family. Absent asks every graph.
- `depth` (integer, optional): How many calls away the walk goes, `CALLERS_DEPTH` for `callers` and `CALLEES_DEPTH` for
  `callees` by default. 0 follows no call.
- `min_confidence` (integer, optional): The least confidence, 0 to 100, a call needs to be taken.
- `max_symbols` (integer, optional): Caps the symbols the walk reaches, its root included, `WALK_MAX_SYMBOLS` by
  default, and 0 sets no cap.

### CallPathParams

- `from` (string, required): Selects the symbol the path starts at.
- `from_path_prefix` (string, optional): As `path_prefix` of [`WalkParams`](#walkparams), for `from`.
- `from_line` (integer, optional): As `line` of [`WalkParams`](#walkparams), for `from`.
- `to` (string, required): Selects the symbol the path ends at.
- `to_path_prefix` (string, optional): As `path_prefix` of [`WalkParams`](#walkparams), for `to`.
- `to_line` (integer, optional): As `line` of [`WalkParams`](#walkparams), for `to`.
- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `depth` (integer, optional): How many calls long a path may be, `CALL_PATH_DEPTH` by default. 0 follows no call.
- `min_confidence` (integer, optional): The least confidence, 0 to 100, a call needs to be taken.
- `max_symbols` (integer, optional): Caps the symbols the search reaches, `CALL_PATH_MAX_SYMBOLS` by default, and 0 sets
  no cap.

### ReferencesParams

- `name` (string, required): Selects the symbol.
- `path_prefix` (string, optional): As in [`WalkParams`](#walkparams).
- `line` (integer, optional): As in [`WalkParams`](#walkparams).
- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `min_confidence` (integer, optional): The least confidence, 0 to 100, a site needs to be taken.
- `access` ([`AccessParam`](#accessparam), optional): Keeps the uses of one access and drops the calls: `read` also
  keeps the updates and the uses as a type, and `write` the updates.
- `limit` (integer, optional): Caps the sites answered, `REFERENCES` by default, and 0 answers the counts alone.

### AccessParam

How a site touches a symbol. A call reads it.

- `read`: Reads the symbol.
- `write`: Assigns the symbol without reading it.
- `update`: Reads and assigns the symbol.
- `type`: Names the symbol as a type, which reads no value.

### ImplementationsParams

- `name` (string, required): Selects the type.
- `path_prefix` (string, optional): As in [`WalkParams`](#walkparams).
- `line` (integer, optional): As in [`WalkParams`](#walkparams).
- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `depth` (integer, optional): How many levels below the root the search goes, `IMPLEMENTATIONS_DEPTH` by default. 0
  finds none.
- `limit` (integer, optional): Caps the implementors answered, `IMPLEMENTATIONS` by default, and 0 answers the total
  alone.

### DefinitionsParams

- `name` (string, required): The name to look up.
- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `from` (string, optional): The file the lookup asks from: definitions in its directory rank first.
- `limit` (integer, optional): Caps the definitions answered, `DEFINITIONS` by default, and 0 answers the total alone.

### SourcesParams

- `name` (string, required): The name to look up.
- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `from` (string, optional): The file the lookup asks from: definitions in its directory rank first.
- `limit` (integer, optional): Caps the definitions answered, `DEFINITIONS` by default, and 0 answers the total alone.
- `max_lines` (integer, optional): Caps the lines of each definition, its doc comment included. 0 answers every line.
- `max_line_bytes` (integer, optional): Caps the bytes of each line. 0 answers lines whole.

### SearchParams

- `query` (string, required): The words to look for.
- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `limit` (integer, optional): Caps the hits answered, `SEARCH` by default, and 0 answers the counts alone.

### OutlineParams

- `file` (string, required): The root-relative path of the file.

### DependentsParams

- `file` (string, required): The root-relative path of the file.
- `depth` (integer, optional): How many includes away the search goes, `DEPENDENTS_DEPTH` by default. 0 finds none.
- `max_files` (integer, optional): Caps the files answered, `DEPENDENTS` by default, and 0 sets no cap.

### SegmentParams

- `file` (string, required): The root-relative path of the file.
- `start_line` (integer, optional): The first line of the segment. Absent or 0 takes the file's first line.
- `end_line` (integer, optional): The last line of the segment. Absent or 0 takes the file's last line.
- `depth` (integer, optional): How many calls away each symbol's blast goes, `SEGMENT_DEPTH` by default. 0 follows no
  call.
- `min_confidence` (integer, optional): The least confidence, 0 to 100, a call needs to be taken.
- `max_symbols` (integer, optional): Caps the symbols each blast reaches, `WALK_MAX_SYMBOLS` by default, and 0 sets no
  cap.

### ChangeParams

- `regions` (array of [`RegionParams`](#regionparams), required): The changed ranges.
- `depth` (integer, optional): How many calls away each changed symbol's walk goes, `SEGMENT_DEPTH` by default. 0
  follows no call.
- `min_confidence` (integer, optional): The least confidence, 0 to 100, a call needs to be taken.
- `max_symbols` (integer, optional): Caps the symbols each walk reaches, `WALK_MAX_SYMBOLS` by default, and 0 sets no
  cap.

### RegionParams

- `file` (string, required): The root-relative path of the file.
- `start_line` (integer, optional): The first line of the range. Absent or 0 takes the file's first line.
- `end_line` (integer, optional): The last line of the range. Absent or 0 takes the file's last line.

### DeadCodeParams

- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `kinds` (array of [`SymbolKind`](#symbolkind), optional): The kinds to judge. Every kind when absent or empty.
- `path_prefix` (string, optional): Judges only the definitions in files under this path.
- `limit` (integer, optional): Caps the definitions answered, `DEAD_CODE` by default, and 0 answers the counts alone.

### ResolutionParams

- `lang` (string, optional): Names the graph to ask. Absent asks every graph.
- `limit` (integer, optional): How many unbound call sites of each graph to list, `UNBOUND` by default, and 0 answers
  the counts alone.

### ListParams

- `path` (string, optional): The directory to list. Absent lists the root.
- `depth` (integer, optional): How many levels to descend, `LIST_DEPTH` when absent, and 0 is the whole subtree.
- `limit` (integer, optional): Caps the entries answered, `LIST_LIMIT` when absent, and 0 sets no cap.
- `collapse` (array of string, optional): Globs naming the directories listed without their entries.
- `exclude` (array of string, optional): Globs naming the entries left out.

### GlobParams

- `globs` (array of string, required): The globs a path must match.
- `exclude` (array of string, optional): Globs naming the paths left out, a directory with everything beneath it.
- `limit` (integer, optional): Caps the paths answered. Absent or 0 sets no cap.

### FuzzyParams

- `query` (string, required): The text to rank paths by.
- `globs` (array of string, optional): Keep the paths they match. Absent keeps every path.
- `exclude` (array of string, optional): Globs naming the paths left out, a directory with everything beneath it.
- `limit` (integer, optional): Caps the matches answered, `FUZZY_LIMIT` by default, and 0 sets no cap.

### GrepParams

- `patterns` (array of string, required): Regular expressions; a line matches when any of them does.
- `case_insensitive` (boolean, optional): Ignores case. False when absent.
- `globs` (array of string, optional): Keep the files they match. Absent keeps every file.
- `exclude` (array of string, optional): Globs naming the files left out.
- `context` (integer, optional): How many lines around each match to answer.
- `limit` (integer, optional): Caps the matching lines answered, `GREP_LIMIT` when absent, and 0 sets no cap.
- `count_all` (boolean, optional): Counts every matching line and file past the limit, into `total_lines` and
  `total_files`, which reads the rest of the tree.
- `max_line_bytes` (integer, optional): Caps the bytes of each line answered, `GREP_MAX_LINE_BYTES` by default, and 0
  answers lines whole.
- `max_file_bytes` (integer, optional): The size past which a file is not searched, `GREP_MAX_FILE_BYTES` by default,
  and 0 searches every size.
- `id` (integer, optional): Names the grep for a cancel.
- `binding_id` (integer, optional): Names the grep for a cancel `by` `binding_id`, apart from `id`.

## Answers of the handle

### About

- `abi` (integer, always): The ABI version, `ABI_VERSION`.
- `version` (string, always): The library's version.
- `methods` (array of string, always): The methods the library answers.
- `languages` (array of string, always): The languages the library ships, as [Languages](#languages) names them.

### StatsAnswer

- `entries` (integer, always): The entries the index holds, directories included.
- `threads` (integer, always): The threads the index runs on: the build's `threads`, or the ambient pool's size when it
  named none.
- `languages` (array of [`LanguageFiles`](#languagefiles), always): What the build made of each language's files.
- `graphs` (array of [`GraphStats`](#graphstats), always): What each family's graph holds.
- `content_index` ([`ContentIndexView`](#contentindexview), optional): Absent when the build made no content index.
- `content_index_failure` (string, optional): Why the build made no content index though it was asked for one; a build
  tries again, and a refresh does not. Absent otherwise.
- `errors` ([`Listed`](#listed), always): The failures the build collected rather than stopped on.
- `watch` ([`WatchView`](#watchview), always): Whether the index follows its tree.
- `stopped_build` ([`StoppedView`](#stoppedview), optional): The answer reads a stopped build's index, so `languages`
  and `graphs` are empty. Absent after a build that finished.

### StoppedView

A cancel stopped the last build, so the handle holds its index and no graph, and an answer that carries this lacks what
that index lacks: a path it does not name may exist. The next build replaces it. It answers no refresh and follows
nothing.

- `entries` (integer, always): The entries the index holds. Which ones depends on where the walk's threads stood.
- `unvisited` (integer, always): The entries the walk listed and the cancel turned away: a floor on what the index
  lacks, since one that is a directory holds more. 0 when the cancel turned nothing away.

### LanguageFiles

- `name` (string, always): The language.
- `family` (string, always): The family whose graph holds it.
- `claimed` (integer, always): The files the language was the first to claim, of those the ignore rules kept.
- `read` (integer, always): The claimed files read as text.
- `cached` (integer, always): The read files whose extraction the content cache answered.
- `held` (integer, always): The read files the family's graph holds.

### GraphStats

The fields of [`Stats`](#stats), and:

- `name` (string, always): The family.

### Stats

- `files` (integer, always): The files the graph holds.
- `files_without_symbols` (integer, always): The files it holds with no definition: their language found none, or could
  not parse them.
- `symbols` (integer, always): The definitions it holds.
- `kinds` (object, always): The definitions of each [`SymbolKind`](#symbolkind), keyed by its label, every kind the
  graph holds none of left out.
- `calls` (integer, always): The calls written by name inside a definition.
- `local_calls` (integer, always): The calls of a definition local to the caller's body.
- `uses` (integer, always): The uses of a name outside a call inside a definition: the four counts below together.
- `reads` (integer, always): The uses that read the value their name binds.
- `writes` (integer, always): The uses that assign the value without reading it.
- `updates` (integer, always): The uses that read and assign the value.
- `types` (integer, always): The uses that name it as a type.
- `includes` (integer, always): The files the held files include, once for each file and each file it includes.

### ContentIndexView

- `files` (integer, always): The files the index holds.
- `unindexed` (integer, optional): The text files the index holds nothing of, because their read failed; a `grep` that
  selects one reads it. Absent at 0.
- `trigrams` (integer, always): The distinct trigrams it holds.
- `bytes` (integer, always): The bytes it takes.

### Listed

The failures a build or a refresh collected rather than stopped on.

- `total` (integer, always): Every failure.
- `listed` (array of [`StoredError`](#storederror), always): The first of them, at most `ERRORS_LISTED`.

### StoredError

- `kind` ([`FailureKind`](#failurekind), always): What failed.
- `path` (string, optional): The path that failed, where one did, as the system spells it. One in the tree starts with
  the root as `open` was given it.
- `file` (string, optional): That path relative to the root, which `paths` of [`RefreshParams`](#refreshparams) takes.
  Empty for the root itself; absent for a path outside the tree, such as a cache directory. A name that is not UTF-8
  reads with replacement characters here as in `path`, so `file` does not name it to a refresh.
- `message` (string, always): Why.

### WatchView

- `following` (boolean, always): Whether a watcher refreshes the index from each change the operating system reports.
  When false, the index takes a change in only through `refresh` or another build.
- `reason` (string, optional): Why the watch a build asked for is off: it could not start, or it stopped. Absent while
  the watch follows the tree, and when the build asked for no watch.
- `restarts` (integer, optional): The watchers started over a refresh of the whole tree, because the one before dropped
  changes, stopped, met the inotify limit, or saw the root deleted or moved. Absent when none was.

### RefreshAnswer

- `walked` (integer, always): The parts of the tree walked again.
- `tree` (boolean, always): Whether the part walked was the whole tree.
- `added` (integer, always): The regular files the index holds now and did not hold before.
- `removed` (integer, always): The regular files the index held before and does not hold now.
- `extracted` (integer, always): The files read, new or changed, whose extraction went into a graph.
- `resolved_again` (integer, always): The unchanged files read and resolved again, because a question their resolution
  asked of the tree has another answer now, or their family's configuration changed.
- `dropped` (integer, optional): The files the graphs held and hold no more. Absent when every graph was built again.
- `reconfigured` (array of string, always): The families whose resolver read another configuration than it read before
  and derived another from it, so that every file of theirs was resolved again.
- `rebuilt` (boolean, always): Whether every graph was built again from every file.
- `widened` ([`WidenedView`](#widenedview), optional): Present when the paths came to more parts than a refresh walks
  apart, so it walked the whole tree.
- `errors` ([`Listed`](#listed), always): The failures the refresh collected; `stats` lists those the tree fails now.
- `watch` ([`WatchView`](#watchview), always): Whether the index follows its tree.

### WidenedView

- `named` (integer, always): The paths the refresh was named.
- `parts` (integer, always): The parts those paths came to, more than `limit`.
- `limit` (integer, always): The most parts a refresh walks apart.

### CancelAnswer

- `stopped` (boolean, always): Whether a call in flight was stopped. A named call that has not entered the library
  starts stopped, within `CANCELLED_AHEAD_LIMIT`. A grep that had already opened its last file answers whole, with
  `cancelled` false, and a refresh past its walk finishes.

### FileView

The answer of a query about one file, with the fields of the answer named beside the method when `held` is true.

- `file` (string, always): The file asked about.
- `graph` (string, optional): The graph of the language that claims the file, absent when none does or when `missing`
  or `directory` is present. Present beside a `held` of false, that graph holds no record of the file.
- `held` (boolean, always): Whether that graph holds the file.
- `missing` (boolean, optional): Present, and true, when the index holds no such path.
- `directory` (boolean, optional): Present, and true, when the index holds the path as a directory: pass one of its
  files instead, which `list` lists.
- `symlink` ([`SymlinkView`](#symlinkview), optional): Present when the index holds the path as a symlink, which no
  graph holds whatever it points at: pass its `path` instead.
- `declined` ([`DeclinedView`](#declinedview), optional): The language that leaves the held file out of its graph, a
  file of its kind it does not claim. A held file with no `graph` and no `declined` is one no language claims.

### SymlinkView

- `target` (string, optional): Where the link points, as the link stores it: relative to the link's directory unless
  absolute, and possibly outside the tree. Absent when the walk could not read it or it is not valid UTF-8.
- `path` (string, optional): The held entry the target names, as the index spells it. Absent when the target leaves
  the root, names nothing the index holds, or passes through a symlink on its way, which the index holds nothing
  below.
- `kind` (string, optional): What that entry is, as [`ListedView`](#listedview)'s `kind` names it. Present with `path`.

### DeclinedView

- `language` (string, always): The first language that leaves the file out.
- `reason` (string, always): Why it leaves the file out, as the language states it.

## Answers of a graph

### Matches

The answer of a query over the graphs.

- `matches` (array of [`Graphed`](#graphed), always): One part for each graph the name selects a symbol in, or for the
  one graph `lang` names, in the order of the engine's graphs.
- `read_as` (string, optional): The name the parts answer for when a qualified name selects no symbol as written: the
  name read with each `.` as `::`, or else each `::` as `.`. Absent for a name that selects as written, and for
  `search`, `dead_code` and `resolution`.
- `from_read_as` (string, optional): The reading of `from` of a `call_path`, each end read on its own.
- `to_read_as` (string, optional): The reading of `to` of a `call_path`.
- `unmatched` ([`Unmatched`](#unmatched), optional): Why `matches` is empty when the name, or its reading, names
  definitions of the kinds the method selects and picks none of them, names definitions none of which is of such a
  kind, or names no definition while names one edit from it do.
- `from_unmatched` ([`Unmatched`](#unmatched), optional): The same for `from` of a `call_path`.
- `to_unmatched` ([`Unmatched`](#unmatched), optional): The same for `to` of a `call_path`.

### Graphed

The fields of the method's answer, and:

- `graph` (string, always): The family whose graph answered.

### Unmatched

- `kind` ([`UnmatchedKind`](#unmatchedkind), always): How the name failed to select.
- `total` (integer, always): How many definitions the name names so.
- `listed` (array of [`GraphDefinition`](#graphdefinition), always): The first of them, at most `UNMATCHED_LISTED`.
- `kinds` (object, optional): For `other_kind`, how many of them are of each [`SymbolKind`](#symbolkind), keyed by its
  label. Absent for every other kind.

### GraphDefinition

- `graph` (string, always): The family whose graph holds the definition.
- `definition` ([`Definition`](#definition), always): The definition.

### Definition

- `name` (string, always): The identifier as written.
- `qualified_name` (string, always): The identifier qualified by what encloses it, as the language writes it.
- `owner` (string, optional): The type or namespace that declares a member.
- `kind` ([`SymbolKind`](#symbolkind), always): What the definition is.
- `file` (string, always): The root-relative path of the file that holds it.
- `line` (integer, always): The one-based line it starts on.
- `end_line` (integer, always): The one-based line it ends on, never before `line`.
- `signature` (string, optional): The declaration up to its body, where the language keeps one.
- `exported` (boolean, always): Whether the definition is visible outside its module.
- `flags` (array of [`SymbolFlag`](#symbolflag), optional): Absent when none is set.

### Candidate

A symbol a name also selects that the answer passed over.

- `symbol` ([`Definition`](#definition), always): The symbol.
- `count` (integer, always): What the query ranked it by: its callers, its sites or its direct implementors. A
  symbol whose calls or uses its graph does not record answers 0 callers or sites and ranks last, whoever uses it.

### Reached

- `symbol` ([`Definition`](#definition), always): The symbol reached.
- `depth` (integer, always): How many calls away it is.
- `confidence` (integer, always): How sure the resolution of the call is, 0 to 100.
- `line` (integer, always): The line of the call.
- `through` ([`Definition`](#definition), optional): In a walk over callers, the method the call binds where it reaches
  the called method only through it: a method of the same name that an interface, a trait or a base class of the called
  method's owner declares, so the call runs whichever implementation the value holds. Absent for a call that binds the
  called symbol itself.

### CallersView

The fields of [`Walk`](#walk), and:

- `band` ([`ReachBand`](#reachband), always): How wide the reach is, graded on the callers in `reached`, so a walk its
  bounds cut reads lower.

### Walk

- `root` ([`Definition`](#definition), always): The symbol the walk started from.
- `passed_over` (integer, always): The other symbols the name selects, of which `root` was chosen.
- `candidates` (array of [`Candidate`](#candidate), optional): The most called of those, the most first.
- `reached` (array of [`Reached`](#reached), always): The symbols reached.
- `truncated` (boolean, always): The symbol cap stopped the walk.
- `turned_away` (integer, always): The symbols the cap kept out.
- `unexpanded` (integer, always): The symbols reached whose own calls were not followed: those at the depth bound, the
  root at a depth of 0, and those the symbol cap left.
- `below_confidence` (integer, always): The calls under the confidence floor.
- `unresolved` (integer, always): The calls that bound no symbol the graph holds.
- `ambiguous` (integer, optional): For a walk over callers, the calls of the root's name that bound no definition
  because several definitions of the name may be meant: any of them may call the root, and none is in `reached`. Absent
  when none.
- `file_scope` (array of [`FileScope`](#filescope), optional): For a walk over callers, the files whose top level calls
  or uses the root's name and sees the root, in path order.
- `self_call` ([`SelfCall`](#selfcall), optional): The root's call of itself, which `reached` does not list.

### FileScope

- `file` (string, always): A file whose top level, outside every definition, calls or uses the name.
- `confidence` (integer, always): How the file sees the symbol: 80 in the same file, 75 in its package, 70 through an
  include; 85 where the top level reads the name through the body that types the value, as Ruby's pasted bodies do. In
  a `references` part a member's file that sees it the first three ways is 15, a guess, as such a name keeps no
  receiver.

### SelfCall

- `confidence` (integer, always): How sure the resolution of the call is, 15 to 95.
- `line` (integer, always): The one-based line of the call, in the symbol's own file.

### CallPath

- `from` ([`Definition`](#definition), always): The symbol the path starts at.
- `to` ([`Definition`](#definition), always): The symbol the path ends at.
- `found` (boolean, always): Whether a path exists within the bounds.
- `hops` (array of [`Reached`](#reached), always): The path.
- `truncated` (boolean, always): The symbol cap stopped the search.
- `turned_away` (integer, always): The symbols the cap kept out.
- `at_depth_bound` (integer, always): The callers other than `from` the search reached at the depth bound, whose own
  callers it did not look at: a longer path may pass through them. Unlike `unexpanded`, it counts nothing the symbol
  cap left, which `truncated` reports.
- `below_confidence` (integer, always): The calls under the confidence floor.
- `passed_over` ([`PassedOver`](#passedover), always): The other symbols each end's name selects.
- `candidates` ([`Ends`](#ends), optional): The most called of each end's passed-over symbols. Absent when neither end
  passed one over.

### PassedOver

- `from` (integer, always): The symbols `from` passed over.
- `to` (integer, always): The symbols `to` passed over.

### Ends

- `from` (array of [`Candidate`](#candidate), always): Those of `from`.
- `to` (array of [`Candidate`](#candidate), always): Those of `to`.

### ReferencesView

The fields of [`References`](#references), and:

- `type_uses_unrecorded` (boolean, optional): Present, and true, when `symbol` is a type and its graph records no use of
  a type where code names it as one: an empty `sites` then does not show that nothing uses the type.
- `data_uses_unrecorded` (boolean, optional): Present, and true, when `symbol` is a field, a variable or a constant of a
  kind whose reads and writes its graph records nowhere: an empty `sites` then does not show that nothing uses it.

### References

- `symbol` ([`Definition`](#definition), always): The symbol.
- `passed_over` (integer, always): The other symbols the name selects.
- `candidates` (array of [`Candidate`](#candidate), optional): The most used of those, the most first.
- `sites` (array of [`Reference`](#reference), always): The sites.
- `total` (integer, always): The sites that pass the confidence floor and the access kept, before the limit.
- `below_confidence` (integer, always): The sites under the confidence floor, and the files of `file_scope` under it.
- `counts` ([`SiteCounts`](#sitecounts), always): The sites that pass the confidence floor, by kind, whatever access is
  kept.
- `ambiguous` (integer, always): The calls of the symbol's name that bound no definition because several definitions of
  the name may be meant, and the uses of a member's name on a value whose type the graph could not follow.
- `file_scope` (array of [`FileScope`](#filescope), optional): The files whose top level calls or uses the symbol's name
  and sees the symbol, in path order. They are no sites: the graph keeps no line for them and not how they touch the
  symbol, so neither `limit` nor `access` bounds them. Absent when none.

### Reference

- `site_of` ([`Definition`](#definition), always): The definition the site sits in.
- `line` (integer, always): The line of the site.
- `confidence` (integer, always): How sure the resolution of the site is, 0 to 100.
- `call` (boolean, always): A call rather than a use.
- `access` ([`AccessParam`](#accessparam), always): How the site touches the symbol.

### SiteCounts

- `calls` (integer, always): The calls.
- `reads` (integer, always): The other uses that read the symbol.
- `writes` (integer, always): The uses that assign it without reading it.
- `updates` (integer, always): The uses that read and assign it.
- `types` (integer, always): The uses that name it as a type.

### DefinitionsView

- `total` (integer, always): Every definition the name selects, before the limit.
- `found` (array of [`FoundView`](#foundview), always): The definitions answered.

### FoundView

- `definition` ([`Definition`](#definition), always): The definition.
- `nearness` ([`Nearness`](#nearness), always): Where it sits from the file the lookup asks from.
- `source` ([`Source`](#source), optional): The definition's text, which only `sources` answers.
- `error` (string, optional): Why `sources` could not read the definition's text.

### Source

- `doc_line` (integer, always): The first line of the doc comment directly above the definition, or the definition's
  own line when there is none.
- `first_line` (integer, always): The first line `text` holds: `doc_line`, or a later one when the line cap left out the
  head of the doc comment.
- `lines` (integer, always): How many lines `text` holds from `first_line`.
- `text` (string, always): The lines, each without its line ending, joined by a newline.
- `cut_lines` (array of [`CutLine`](#cutline), always): The lines of `text` the byte cap cut.
- `resized` (boolean, always): The file's size differs from the size the build read, so the lines may have moved.

### CutLine

- `line` (integer, always): The line cut.
- `bytes` (integer, always): Its length before the cut.

### Search

- `hits` (array of [`Hit`](#hit), always): The definitions found.
- `total` (integer, always): The definitions that share a word with the query or that it names, before the limit.
- `named` (integer, always): Those of `total` whose name matched, rather than only their file's path.
- `unmatched_words` (array of string, always): The words of the query no definition holds.
- `dropped_words` (integer, always): The distinct words of the query past the first eight, which were not looked for.
- `fuzzy_total` (integer, optional): What the fill-up by characters found before the limit, when it ran.

### Hit

- `definition` ([`Definition`](#definition), always): The definition.
- `score` (number, always): The rank, higher first. Platforms agree on it within a relative 1e-12.
- `exact` (boolean, always): The name is the query as written.
- `fuzzy` (boolean, always): The fill-up by characters found it.

### Implementations

- `root` ([`Definition`](#definition), always): The type asked about.
- `passed_over` (integer, always): The other types the name selects.
- `candidates` (array of [`Candidate`](#candidate), optional): The most implemented of those, the most first.
- `found` (array of [`Implementor`](#implementor), always): The implementors answered.
- `total` (integer, always): The implementors within the depth bound, before the limit.
- `unexpanded` (integer, always): The implementors at the depth bound whose own implementors were not followed, and the
  type asked about at a depth of 0.

### Implementor

- `symbol` ([`Definition`](#definition), always): The implementor.
- `depth` (integer, always): How many levels below the root it is.

### OutlineView

- `symbols` (array of [`Definition`](#definition), always): The definitions of the file.

### Dependents

- `found` (array of [`Dependent`](#dependent), always): The files answered.
- `truncated` (boolean, always): The file cap stopped the search.
- `turned_away` (integer, always): The files the cap kept out.
- `unexpanded` (integer, always): The files reached whose own dependents were not followed: those at the depth bound,
  the file asked about at a depth of 0, and those the file cap stopped at.

### Dependent

- `file` (string, always): The file.
- `depth` (integer, always): How many includes away it is.

### Segment

The fields of [`Lines`](#lines), and:

- `symbols` (array of [`SegmentSymbol`](#segmentsymbol), always): The symbols the segment overlaps.
- `includes` (array of string, always): The files the file includes.
- `included_by` (array of string, always): The files that include the file.

### Lines

- `start_line` (integer, always): The first line of the segment.
- `end_line` (integer, always): The last line of the segment. Where the params left it open or named 4294967295, the
  last line a symbol of the file reaches, and never before `start_line`.

### SegmentSymbol

- `definition` ([`Definition`](#definition), always): The symbol.
- `callers` ([`DirectCalls`](#directcalls), always): Its direct callers.
- `users` ([`DirectCalls`](#directcalls), optional): The direct users of a field, a variable or a constant: the symbols
  that call it or whose code reads or writes it. Absent for any other kind, and for one with no user.
- `ambiguous` (integer, optional): The calls of its name a walk over its callers counts as `ambiguous` of
  [`Walk`](#walk). Absent when none.
- `callees` ([`DirectCalls`](#directcalls), always): Its direct callees.
- `unresolved` (integer, always): Its calls that bound no symbol the graph holds.
- `blast` ([`Blast`](#blast), always): What a walk over its callers reached.

### DirectCalls

- `listed` (array of [`Reached`](#reached), always): The first of them.
- `total` (integer, always): Those that pass the confidence floor.
- `below_confidence` (integer, always): Those under it.

### Blast

- `reach` (integer, always): The callers reached, the symbol itself not among them.
- `by_depth` (array of integer, always): The callers reached at each depth, the direct callers first.
- `affected_files` (integer, always): The other files that hold a caller reached or include the symbol's file.
- `band` ([`ReachBand`](#reachband), always): How wide the reach is.
- `truncated` (boolean, always): The symbol cap stopped the walk.
- `turned_away` (integer, always): The symbols the cap kept out.
- `unexpanded` (integer, always): The symbols reached whose own callers were not followed: those at the depth bound, the
  symbol itself at a depth of 0, and those the symbol cap left.
- `below_confidence` (integer, always): The calls under the confidence floor.

### ChangeView

The fields of [`Change`](#change), and why no graph holds each region `unheld_regions` counts:

- `missing_regions` (integer, always): The regions whose path the index does not hold.
- `symlink_regions` (integer, always): The regions whose path the index holds as a symlink.
- `directory_regions` (integer, always): The regions whose path the index holds as a directory: pass its files
  instead, which `list` lists.
- `unclaimed_regions` (integer, always): The regions of held files that no language claims or declines.
- `declined` (array of [`DeclinedRegionsView`](#declinedregionsview), always): The regions of held files that a
  language leaves out of its graph, per language and reason, in the order the regions first name them. The rest of
  `unheld_regions` are regions of held files that the graph claiming them holds no record of.

### DeclinedRegionsView

The fields of [`DeclinedView`](#declinedview), and:

- `regions` (integer, always): The regions that name a file the language leaves out for that reason.

### Change

- `changed_symbols` (integer, always): The symbols the regions overlap.
- `changed_files` (integer, always): The files whose symbols the regions overlap.
- `unheld_regions` (integer, always): The regions whose file no graph holds.
- `regions_outside_symbols` (integer, always): The regions of held files that overlap no symbol.
- `footprint` (integer, always): The symbols the walks over the changed symbols' callers reach, each once.
- `footprint_files` (integer, always): The files that hold them.
- `transitive_sum` (integer, always): The reaches of the changed symbols, summed: what exceeds `footprint` is where they
  overlap.
- `hotspots` (array of [`Hotspot`](#hotspot), always): The symbols two or more changed symbols reach, cut to a cap the
  engine holds.
- `total_hotspots` (integer, always): Every such symbol, before the cut.
- `seeds` (array of [`Seed`](#seed), always): The changed symbols, cut to a cap the engine holds.
- `truncated_seeds` (integer, always): The changed symbols whose walk the symbol cap stopped.

### Hotspot

- `symbol` ([`Definition`](#definition), always): The symbol.
- `hit_count` (integer, always): The changed symbols that reach it.
- `min_depth` (integer, always): How many calls away from the nearest of them it is.
- `also_changed` (boolean, always): It is itself a changed symbol.

### Seed

- `symbol` ([`Definition`](#definition), always): The changed symbol.
- `reach` (integer, always): The callers it reaches.
- `band` ([`ReachBand`](#reachband), always): How wide the reach is.
- `caller_count` (integer, always): Its direct callers that pass the confidence floor.
- `user_count` (integer, optional): The direct users of a field, a variable or a constant that pass the confidence
  floor. Absent when none, and for any other kind.
- `ambiguous` (integer, optional): The calls of its name a walk over its callers counts as `ambiguous` of
  [`Walk`](#walk). Absent when none.
- `consumers` (array of [`Reached`](#reached), always): Its direct users or callers while they are few, else empty.
- `consumer_dirs` (array of [`DirCount`](#dircount), always): Those users or callers by directory when they are many.
- `truncated` (boolean, always): The symbol cap stopped the walk.
- `turned_away` (integer, always): The symbols the cap kept out.
- `unexpanded` (integer, always): The symbols reached whose own callers were not followed: those at the depth bound, the
  symbol itself at a depth of 0, and those the symbol cap left.
- `below_confidence` (integer, always): The calls under the confidence floor.

### DirCount

- `dir` (string, always): The directory.
- `count` (integer, always): How many it holds.

### DeadCode

- `found` (array of [`Dead`](#dead), always): The dead definitions, in path order, at most the limit.
- `total` (integer, always): The dead definitions, before the limit.
- `live` (integer, always): The definitions judged and found reached.
- `excluded` ([`Excluded`](#excluded), always): The definitions left unjudged, by reason.

### Dead

- `definition` ([`Definition`](#definition), always): The definition.
- `spelled` (boolean, always): Whether a call or a use the graph holds spells its name: they were resolved, and none
  reaches it.

### Excluded

Each definition counts once, under the first reason that holds for it.

- `kind` (integer, always): Of a kind the query did not ask for.
- `test` (integer, always): Test code, which a test runner calls.
- `generated` (integer, always): Generated code.
- `entry` (integer, always): Entry points, which run with no call that spells their name.
- `vendored` (integer, always): Vendored code.
- `unobservable` (integer, always): Of a kind that no call and no use the language records shows to be used.
- `uncertain` (integer, always): Definitions that a call or a use of their name may reach which binds no definition, or
  binds one only by proximity.
- `dispatched` (integer, always): Methods a call through another type may reach.
- `reflected` (integer, always): Definitions the language's runtime reads or writes by their name, through reflection.

### Resolution

- `sites` (integer, always): The call sites resolved.
- `bound` (array of [`Bound`](#bound), always): The sites that bound a definition, the surest first, every confidence no
  site bound at left out.
- `edges` (integer, always): The pairs of a caller and the definition it calls, each pair once.
- `misses` ([`Misses`](#misses), always): The sites that bound none, by reason.
- `unbound` (array of [`Unbound`](#unbound), always): Sites that bound none, in path order, at most the limit.
- `unlisted` (integer, always): The sites that bound none and that `unbound` does not list.

### Bound

- `confidence` (integer, always): The confidence of the tier that bound them.
- `sites` (integer, always): The sites it bound.

### Misses

- `external_import` (integer, always): The sites [`Miss`](#miss) `external_import` names.
- `not_exported` (integer, always): The sites [`Miss`](#miss) `not_exported` names.
- `undefined` (integer, always): The sites [`Miss`](#miss) `undefined` names.
- `not_admitted` (integer, always): The sites [`Miss`](#miss) `not_admitted` names.
- `ambiguous` (integer, always): The sites [`Miss`](#miss) `ambiguous` names.
- `past_bound` (integer, optional): The sites [`Miss`](#miss) `past_bound` names. Absent when none.
- `through_a_receiver` (integer, always): Of the undefined and the not-admitted sites, those made through any receiver.

### Unbound

- `caller` ([`Definition`](#definition), always): The definition whose body makes the call.
- `line` (integer, always): The one-based line of the call.
- `callee` (string, always): The callee as the call writes it.
- `receiver` (string, optional): The receiver as the call writes it, empty for a value the caller declares. Absent for a
  call with none.
- `miss` ([`Miss`](#miss), always): Why it bound none.

## Answers of the file index

### ListView

- `path` (string, always): The directory listed.
- `entries` (array of [`ListedView`](#listedview), always): Its entries.
- `truncated` (boolean, always): The limit left entries out.
- `excluded` (integer, always): The entries `exclude` left out.
- `total` (integer, always): The entries the index holds under the directory, at every depth.
- `stopped_build` ([`StoppedView`](#stoppedview), optional): The answer reads a stopped build's index. Absent after a
  build that finished.

### ListedView

- `path` (string, always): The entry.
- `kind` ([`EntryKind`](#entrykind), always): What it is.
- `size` (integer, optional): Its size in bytes, where the index holds one.
- `binary` (boolean, optional): Whether it is binary, where the index settled it.
- `target` (string, optional): Where a symlink points.
- `unlisted` (integer, optional): The entries below a directory the listing did not descend into.

### GlobView

- `paths` (array of string, always): The matching paths in index order.
- `truncated` (boolean, always): The limit left paths out.
- `excluded` (integer, optional): The matching paths `exclude` left out before the answer ended. Absent when none.
- `stopped_build` ([`StoppedView`](#stoppedview), optional): The answer reads a stopped build's index. Absent after a
  build that finished.

### FuzzyView

- `matches` (array of [`FuzzyMatchView`](#fuzzymatchview), always): The best-ranked paths, best first.
- `truncated` (boolean, always): The limit left matches out.
- `matched` (integer, optional): The paths that matched the query, those past the limit included. Absent for an empty
  query, which is not counted.
- `stopped_build` ([`StoppedView`](#stoppedview), optional): The answer reads a stopped build's index. Absent after a
  build that finished.

### FuzzyMatchView

- `path` (string, always): The path.
- `score` (integer, always): The rank, higher first.

### GrepView

A candidate is a regular file the filter selected and the exclusion kept. Each lands in exactly one of `searched`,
`binary`, `oversized`, `ruled_out`, `failures` and `unsearched`.

- `files` (array of [`GrepFileView`](#grepfileview), always): The files holding a match, in index order.
- `lines` (integer, always): The matching lines answered, context lines not counted.
- `truncated` (boolean, always): The limit turned a matching line away.
- `total_lines` (integer, optional): The matching lines the searched candidates hold, past the limit included, when
  `count_all` asked.
- `total_files` (integer, optional): The searched candidates that hold a matching line, when `count_all` asked.
- `cancelled` (boolean, always): A cancel stopped the search before it covered every candidate.
- `candidates` (integer, always): Every file this search could have read.
- `excluded` (integer, always): The regular files the filter selected that `exclude` removed.
- `symlinks` (integer, always): The symlinks the filter selected, none of which is searched.
- `searched` (integer, always): The candidates read and covered by this answer.
- `binary` (integer, always): The candidates skipped as binary, by extension or by a NUL in the first 64 KiB.
- `oversized` (integer, always): The candidates larger than `max_file_bytes`.
- `ruled_out` (integer, always): The candidates the content index ruled out unread. 0 does not show that an index
  narrowed the search: with none to ask, or none it could read, the search reads every candidate and answers 0.
- `unsearched` (integer, always): The candidates this answer does not cover, because a match past the limit proved the
  cut or a cancel landed first.
- `failures` (array of [`GrepFailureView`](#grepfailureview), always): The candidates that could not be read.
- `stopped_build` ([`StoppedView`](#stoppedview), optional): The answer reads a stopped build's index, whose candidates
  are the files it holds. Absent after a build that finished.

### GrepFileView

- `path` (string, always): The file.
- `lines` (array of [`GrepLineView`](#greplineview), always): Its lines, in file order.

### GrepLineView

- `number` (integer, always): The line's number in its file, counting from 1.
- `kind` ([`LineKind`](#linekind), always): Whether it matched.
- `text` (string, always): The text without its line ending; bytes that are not UTF-8 read as U+FFFD.
- `cut` ([`CutView`](#cutview), optional): Present when `max_line_bytes` cut the line.

### CutView

- `before` (integer, always): The bytes left out before the text.
- `after` (integer, always): The bytes left out after it.

### GrepFailureView

- `path` (string, always): The file.
- `message` (string, always): Why it could not be read.

## Values

### EntryKind

- `file`: A regular file.
- `dir`: A directory.
- `symlink`: A symbolic link.
- `other`: Anything else.

### FailureKind

Each value is the engine's label. From `cancelled` on, a failure stops the call, so an
[`ErrorCode`](#errorcode) answers it and no entry of `errors` holds it.

- `walk`: The walk failed, for one directory or entry.
- `ignore`: A `.gitignore` exists but could not be read, so the index holds every entry it would have hidden.
- `name`: An entry's name is not valid UTF-8, so it was not indexed, and a directory takes its subtree with it.
- `read`: A file could not be read, so its content is missing; the entry is still indexed.
- `cache`: The content cache failed.
- `content_index`: The content index failed: the build holds what else it made and no content index.
- `cancelled`: A cancel stopped the work.
- `thread_pool`: The index's thread pool could not start.
- `content_ids_exhausted`: The index has no content ids left to number the files a refresh reads.
- `misspelled`: A path given to `refresh` is not spelled from the root with `/` between names.

### LineKind

- `match`: The line matched one of the patterns.
- `context`: The line sits within `context` lines of a match.

### Miss

- `external_import`: The call names an import of a module outside the repository, code the graph does not hold.
- `not_exported`: The receiver names an import of a directory the graph holds, which exports no callable of the name.
- `undefined`: No definition the graph holds has the name as the call writes it: a builtin, or code outside the graph.
- `not_admitted`: Definitions of the name exist, and the call can bind none of them.
- `past_bound`: The graph stopped reading the receiver at a bound, so nothing is known of what the call reaches.
- `ambiguous`: Several definitions of the name may be meant, and neither scope nor nearness picks one.

### Nearness

- `same_directory`: In the directory of the file the lookup asks from.
- `elsewhere`: Anywhere else.

### ReachBand

The highest band whose rule holds answers.

- `low`: Fewer than 10 callers, and not exported with 20 or more.
- `medium`: 10 to 49 callers, and not exported with 20 or more.
- `high`: 50 or more callers, or 20 or more for an exported symbol.
- `critical`: 200 or more callers, or 50 or more for an interface.

### SymbolFlag

- `test`: Test code.
- `entry`: An entry point.
- `generated`: Generated code.
- `vendored`: Vendored code.

### SymbolKind

- `func`: A function.
- `method`: A method.
- `class`: A class.
- `struct`: A struct.
- `ns`: A namespace.
- `iface`: An interface.
- `var`: A variable.
- `const`: A constant.
- `enum`: An enum.
- `alias`: A type alias.
- `trait`: A trait.
- `macro`: A macro.
- `field`: A field.
- `variant`: An enum variant.
- `type`: Any other type.

### UnmatchedKind

- `several`: Several definitions under the path prefix end with the name and it picks none of them.
- `outside_prefix`: The name names definitions outside the path prefix, none under it, or one that the selector looks
  at.
- `off_line`: It names definitions under the path prefix that start off the line and at most one on it, these counting
  and listing those outside the prefix too.
- `one_edit`: It names no definition of any kind, and these are named one edit from it.
- `other_kind`: It names definitions and none is of a kind the method selects: these are every one of them, wherever
  it lies. Only `references` and `implementations` refuse a kind, so only they carry it.
