/**
 * The wire `kartta-ffi` answers: each method's params and answer, field for field as the JSON spells them. A bound
 * left out takes the method's default, and a bound of 0 means what the field says. A count absent at 0 reads as 0.
 *
 * @module
 */

/** Each method of the wire, its params and its answer. */
export type Methods = {
	build: {params: BuildParams; answer: Stats};
	refresh: {params: RefreshParams; answer: Refresh};
	cancel: {params: CancelParams; answer: CancelAnswer};
	stats: {params: NoParams; answer: Stats};
	version: {params: NoParams; answer: Version};
	resolution: {params: ResolutionParams; answer: Matches<Resolution>};
	callers: {params: WalkParams; answer: Matches<Walk>};
	callees: {params: WalkParams; answer: Matches<Walk>};
	call_path: {params: CallPathParams; answer: Matches<CallPath>};
	references: {params: ReferencesParams; answer: Matches<References>};
	definitions: {params: DefinitionsParams; answer: Matches<Definitions>};
	sources: {params: SourcesParams; answer: Matches<Definitions>};
	search: {params: SearchParams; answer: Matches<Search>};
	implementations: {params: ImplementationsParams; answer: Matches<Implementations>};
	dead_code: {params: DeadCodeParams; answer: Matches<DeadCode>};
	outline: {params: OutlineParams; answer: Outline};
	dependents: {params: DependentsParams; answer: Dependents};
	analyze_segment: {params: SegmentParams; answer: Segment};
	analyze_change: {params: ChangeParams; answer: Change};
	list: {params: ListParams; answer: Listing};
	glob: {params: GlobParams; answer: Glob};
	fuzzy: {params: FuzzyParams; answer: Fuzzy};
	grep: {params: GrepParams; answer: Grep};
};

/**
 * What a failure reports. Every code but `closed` is the engine's; `closed` is the binding's own, for a call on a
 * closed handle, and the binding also rejects with `internal` for a failure of the addon or of reading its answer.
 */
export type Code =
	| 'invalid_request'
	| 'invalid_params'
	| 'unknown_method'
	| 'invalid_root'
	| 'not_built'
	| 'building'
	| 'build_failed'
	| 'build_cancelled'
	| 'no_stamps'
	| 'refresh_failed'
	| 'refresh_cancelled'
	| 'invalid_glob'
	| 'invalid_pattern'
	| 'not_found'
	| 'not_a_directory'
	| 'internal'
	| 'closed';

/** The params of a method that takes none. */
export type NoParams = Record<string, never>;

/**
 * The params of `build`. Left empty, a build indexes every language kartta ships, under the tree's own `.gitignore`
 * files, with no content index and no cache.
 */
export type BuildParams = {
	/** Names the build for a cancel; the handle passes it on as it is. */
	id?: number;
	/**
	 * Names the build for a cancel `by` `binding_id`, apart from `id`: a binding over the wire names each call it runs
	 * by one, so the ids its callers give never stop a call of its own. The handle sets it, in place of any the params
	 * give.
	 */
	binding_id?: number;
	/** The languages to build graphs of, by the names `version` lists; absent builds every one, empty none. */
	languages?: string[];
	/** Bounds the threads the index runs on, at least 1; absent takes every core, and 0 rejects with invalid_params. */
	threads?: number;
	/** `in_tree` applies the `.gitignore` files inside the tree, the default; `none` applies no ignore rule. */
	ignore?: 'in_tree' | 'none';
	/** Changes the entry names the walk neither indexes nor descends, editor and OS clutter by default. */
	pruned_names?: SetChange;
	/** Changes the extensions that settle a file as binary unread. */
	binary_extensions?: SetChange;
	/** Builds a trigram index of file content, kept where it says. */
	content_index?: ContentIndex;
	/** A directory per-file data is shared through across builds. */
	content_cache?: string;
	/** Seconds after the build that computed it began that a cache entry expires; absent or 0 is a week. */
	content_cache_ttl_secs?: number;
	/**
	 * Keeps the time each file the build reads last changed, so that a refresh reads again only the files that changed
	 * since and those a change reaches. It holds memory for as long as the index lives: eight bytes a file for its time,
	 * and what each file's resolution asked of the tree, most of it, 32 MiB over a tree of 182,714 entries. On unix the
	 * time is the inode change time, which no write can set back; elsewhere it is the modification time, so an edit that
	 * keeps a file's size and sets its modification time back goes unseen. A file changed within two seconds before the
	 * build began is read again by the next refresh whatever its stamp says. `watch` keeps stamps when this is absent,
	 * and rejects it false with `invalid_params`.
	 */
	stamps?: boolean;
	/**
	 * Follows the tree through the operating system's change notifications: the library refreshes the index from each
	 * change on a thread of its own, as `refresh` does, until the next build or `close`, so no refresh is needed for a
	 * change the notifications report. The build answers once the index holds the changes reported while it was built;
	 * one macOS reports late is taken in after. `watch` in `stats` says whether the index still follows the tree.
	 */
	watch?: boolean;
};

/** The options of {@link Kartta.build}: the params of `build` but the binding id the handle gives each build. */
export type BuildOptions = Omit<BuildParams, 'binding_id'>;

/** Extends a built-in set, or replaces it. */
export type SetChange = {extend: string[]} | {replace: string[]};

/** Where a content index lives: in memory, or in scratch files the file and mapped storages create in `dir`. */
export type ContentIndex = {storage: 'memory'} | {storage: 'file' | 'mapped'; dir: string};

/**
 * The params of `refresh`. Left empty, a refresh walks the whole tree again; one made from a build without `stamps`
 * rejects with `no_stamps`.
 */
export type RefreshParams = {
	/** Names the refresh for a cancel; the handle passes it on as it is. */
	id?: number;
	/**
	 * Names the refresh for a cancel `by` `binding_id`, apart from `id`: a binding over the wire names each call it runs
	 * by one, so the ids its callers give never stop a call of its own. The handle sets it, in place of any the params
	 * give.
	 */
	binding_id?: number;
	/**
	 * The root-relative paths that changed, spelled as the index's answers spell them; absent or null refreshes the
	 * whole tree, and empty walks nothing. A path that starts with `/` or `\`, starts with a drive such as `C:`, or
	 * holds a `\` or a `..` name rejects with `invalid_params`, the message naming which.
	 */
	paths?: string[] | null;
};

/** The options of {@link Kartta.refresh}: the params of `refresh` but the binding id the handle gives each refresh. */
export type RefreshOptions = Omit<RefreshParams, 'binding_id'>;

/** The answer of `refresh`: what it walked again, and what the index and the graphs took in and let go. */
export type Refresh = {
	/** The parts of the tree walked again. */
	walked: number;
	/** Whether the part walked was the whole tree. */
	tree: boolean;
	/** The regular files the index holds now and did not hold before. */
	added: number;
	/** The regular files the index held before and does not hold now. */
	removed: number;
	/** The files read, new or changed, whose extraction went into a graph. */
	extracted: number;
	/**
	 * The unchanged files read and resolved again, because a question their resolution asked of the tree has another
	 * answer now, or their family's configuration changed.
	 */
	resolved_again: number;
	/** The files the graphs held and hold no more; absent when every graph was built again. */
	dropped?: number;
	/**
	 * The language families whose resolver read another configuration than it read before and derived another from
	 * it, such as jsts for TypeScript and JavaScript, so that every file of theirs was resolved again.
	 */
	reconfigured: string[];
	/** Whether every graph was built again from every file. */
	rebuilt: boolean;
	/** Present when the paths came to more parts than a refresh walks apart, so it walked the whole tree. */
	widened?: Widened;
	/** The failures the refresh collected; `stats` lists those the tree fails now. */
	errors: BuildErrors;
	/** Whether the index follows its tree. */
	watch: Watch;
};

/** Whether the index follows its tree, which decides whether an answer can be older than the tree. */
export type Watch = {
	/**
	 * Whether a watcher refreshes the index from each change the operating system reports. When it is false, the index
	 * takes a change in only through `refresh` or another build.
	 */
	following: boolean;
	/**
	 * Why the watch a build asked for is off: it could not start, or it stopped. Absent while the watch follows the
	 * tree, and when the build asked for no watch.
	 */
	reason?: string;
	/**
	 * The watchers the watch started over a refresh of the whole tree, because the one before dropped changes, stopped,
	 * met the inotify limit, or saw the root deleted or moved; absent when none was.
	 */
	restarts?: number;
};

/** How a refresh of named paths came to walk the whole tree. */
export type Widened = {
	/** The paths the refresh was named. */
	named: number;
	/** The parts those paths came to, more than `limit`. */
	parts: number;
	/** The most parts a refresh walks apart. */
	limit: number;
};

/**
 * The params of `cancel`. Naming no call stops every build in flight, running or waiting its turn, every grep and every
 * refresh in flight.
 */
export type CancelParams = {
	/** The id of the build to stop. */
	build?: number;
	/** The id of the grep to stop. */
	grep?: number;
	/** The id of the refresh to stop. */
	refresh?: number;
	/**
	 * The name of a call the ids above are compared with: its `id`, by default, or its `binding_id`. An id by one name
	 * never stops a call by the other.
	 */
	by?: 'id' | 'binding_id';
};

/** The answer of `cancel`. */
export type CancelAnswer = {
	/**
	 * Whether a call in flight was stopped. A named call that has not entered the library starts stopped, unless cancels
	 * that stopped nothing named 1,024 distinct later ids of its kind by the same name before it entered; a grep that
	 * had already opened its last file answers whole, with `cancelled` false, and a refresh past its walk finishes.
	 */
	stopped: boolean;
};

/** The answer of `build` and `stats`. */
export type Stats = {
	/** The entries the index holds, directories included. */
	entries: number;
	/** The threads the index runs on: the build's `threads`, or the ambient pool's size when it named none. */
	threads: number;
	languages: LanguageFiles[];
	graphs: GraphStats[];
	/** Absent when the build made no content index. */
	content_index?: ContentIndexStats;
	/** Why the build made no content index though it was asked for one; a build tries again, and a refresh does not. */
	content_index_failure?: string;
	errors: BuildErrors;
	watch: Watch;
	/** Present when the answer reads a stopped build's index, which leaves `languages` and `graphs` empty. */
	stopped_build?: StoppedBuild;
};

/**
 * A cancel stopped the last build, so the handle holds its index and no graph, and an answer that carries this lacks
 * what that index lacks: a path it does not name may exist.
 */
export type StoppedBuild = {
	/** The entries the index holds. */
	entries: number;
	/**
	 * The entries the walk listed and the cancel turned away: a floor on what the index lacks, since one that is a
	 * directory holds more. 0 when the cancel turned nothing away.
	 */
	unvisited: number;
};

/** What a build made of one language's files. */
export type LanguageFiles = {
	name: string;
	family: string;
	/** The files the language was the first to claim, of those the ignore rules kept. */
	claimed: number;
	/** The claimed files read as text. */
	read: number;
	/** The read files whose extraction the content cache answered. */
	cached: number;
	/** The read files the family's graph holds. */
	held: number;
};

/** What one family's graph holds. */
export type GraphStats = {
	name: string;
	files: number;
	files_without_symbols: number;
	symbols: number;
	/** The symbols of each kind, by its label. */
	kinds: Record<string, number>;
	calls: number;
	local_calls: number;
	/** The uses of a name outside a call inside a definition: `reads`, `writes`, `updates` and `types` together. */
	uses: number;
	reads: number;
	writes: number;
	updates: number;
	types: number;
	includes: number;
};

/** What a content index holds. */
export type ContentIndexStats = {
	files: number;
	/**
	 * The text files the index holds nothing of, because their read failed; a grep that selects one reads it. Absent
	 * at 0.
	 */
	unindexed?: number;
	trigrams: number;
	bytes: number;
};

/** The failures a build collected rather than stopped on: `total` of them, the first of them listed. */
export type BuildErrors = {
	total: number;
	listed: BuildFailure[];
};

/** One failure a build collected. */
export type BuildFailure = {
	/**
	 * `walk`, `ignore`, `name`, `read`, `content_index` or `cache`; a failure of any other kind stops the call and
	 * answers its code.
	 */
	kind: string;
	/** The path that failed, as the system spells it. One in the tree starts with the root as `open` was given it. */
	path?: string;
	/**
	 * That path relative to the root, which `refresh`'s `paths` takes. Empty for the root itself; absent for a path
	 * outside the tree, such as a cache directory.
	 */
	file?: string;
	message: string;
};

/** The answer of `version`. */
export type Version = {
	abi: number;
	version: string;
	methods: string[];
	languages: string[];
};

/**
 * The answer of a query that names a symbol: one part for each graph the name selects a symbol in, or for the one
 * graph the params' `lang` names, in the order of the engine's graphs. Each part names its graph.
 */
export type Matches<T> = {
	matches: T[];
	/**
	 * The name the parts answer for when the params' qualified name selects no symbol as written in the graphs asked:
	 * the name read with each `.` as `::`, or else each `::` as `.`, as `Index.refresh` is read as Rust's
	 * `Index::refresh`. A name that selects as written is never read, and the key is then absent, as it is for `search`,
	 * `dead_code` and `resolution`. A reading the method then answers nothing for leaves `matches` empty.
	 */
	read_as?: string;
	/** The reading of a `call_path`'s `from`, each end read on its own. */
	from_read_as?: string;
	/** The reading of a `call_path`'s `to`, each end read on its own. */
	to_read_as?: string;
	/**
	 * Why `matches` is empty when the name, or its reading, names definitions of the kinds the method selects and picks
	 * none of them, names definitions none of which is of such a kind, or names no definition while names one edit from
	 * it do; absent for a name that matched or for which none holds. `callers`, `callees`, `references`, `implementations`, `definitions` and `sources` carry it.
	 */
	unmatched?: Unmatched;
	/** The same for a `call_path`'s `from`, carried only when that end selects no symbol. */
	from_unmatched?: Unmatched;
	/** The same for a `call_path`'s `to`, carried only when that end selects no symbol. */
	to_unmatched?: Unmatched;
};

/** What a name that selected no symbol names instead, among the definitions of the kinds the method selects. */
export type Unmatched = {
	/**
	 * `several` when several definitions under the path prefix end with the name and it picks none of them, a longer
	 * name or a path prefix holding one picking one; `outside_prefix` when the name names definitions outside the path
	 * prefix, none under it, or one that the selector looks at; `off_line` when it names definitions under the path
	 * prefix that start off the line and at most one on it, these counting and listing those outside the prefix too;
	 * `one_edit` when the name names no definition of any kind, and these are named one edit from it: one character
	 * swapped with the next, dropped, replaced or added, a replacing or added one being an ASCII letter, a digit, `_`
	 * or one the name holds; `other_kind` when the name names definitions and none is of a kind the method selects,
	 * these being every one of them, wherever it lies.
	 */
	kind: string;
	/** How many definitions the name names so. */
	total: number;
	/** The first five of them. */
	listed: GraphDefinition[];
	/** How many of them are of each kind, by its label, for `other_kind`; absent for every other kind. */
	kinds?: Record<string, number>;
};

/** A definition and the graph that holds it. */
export type GraphDefinition = {
	graph: string;
	definition: Definition;
};

/** A symbol a graph holds, where it is defined. */
export type Definition = {
	name: string;
	qualified_name: string;
	owner?: string;
	/** `func`, `method`, `class`, `struct`, `ns`, `iface`, `var`, `const`, `enum`, `alias`, `trait`, `macro`, `field`,
	 * `variant` or `type`. */
	kind: string;
	file: string;
	line: number;
	end_line: number;
	signature?: string;
	exported: boolean;
	/** Those set of `test`, `entry`, `generated` and `vendored`. */
	flags?: string[];
};

/** A symbol a walk reached, and the call that reached it. */
export type Reached = {
	symbol: Definition;
	depth: number;
	confidence: number;
	/** The line of the call. */
	line: number;
	/**
	 * In a walk over callers, the method the call binds where it reaches the called method only through it: a method of
	 * the same name that an interface, a trait or a base class of the called method's owner declares, so the call runs
	 * whichever implementation the value holds. Absent for a call that binds the called symbol itself.
	 */
	through?: Definition;
};

/** A symbol a name also selects that the answer passed over, and the count the query ranked it by. */
export type Candidate = {
	symbol: Definition;
	count: number;
};

/** The params of `callers` and `callees`. */
export type WalkParams = {
	/** Selects the symbol to start from: a bare or a qualified name. */
	name: string;
	/**
	 * Keeps only the symbols in files whose root-relative path starts with it, one file's path or a directory's with
	 * its closing slash; it reaches one of several copies that share a qualified name. Absent looks in every file.
	 */
	path_prefix?: string;
	/**
	 * Keeps only the symbols that start on this one-based line, as a `Definition`'s `line`; it reaches one of several
	 * symbols of the name in one file. Absent looks at every line, and 0 selects no symbol.
	 */
	line?: number;
	/** Names the graph to ask, by a language or its family; absent asks every graph. */
	lang?: string;
	/** How many calls away the walk goes; callers default to 3, callees to 1, and 0 follows no call. */
	depth?: number;
	/** The least confidence, 0 to 100, a call needs to be taken. */
	min_confidence?: number;
	/** Caps the symbols the walk reaches, its root included; 500 by default, and 0 sets no cap. */
	max_symbols?: number;
};

/** What a walk over calls reached in one graph, and what it left out. */
export type Walk = {
	graph: string;
	root: Definition;
	/** The other symbols the name selects, of which `root` was chosen. */
	passed_over: number;
	/** The most called of those, the most first, at most five; `count` is each one's callers. */
	candidates?: Candidate[];
	reached: Reached[];
	/** The symbol cap stopped the walk. */
	truncated: boolean;
	/** The symbols the cap kept out. */
	turned_away: number;
	/**
	 * The symbols reached whose own calls were not followed: those at the depth bound, the root at a depth of 0, and those
	 * the symbol cap left.
	 */
	unexpanded: number;
	/** The calls under the confidence floor. */
	below_confidence: number;
	/** The calls that bound no symbol the graph holds, a construction of a class outside the graph as one. */
	unresolved: number;
	/**
	 * For a walk over callers, the calls of the root's name that bound no definition because several definitions of the
	 * name may be meant: any of them may call the root, and none is in `reached`. It is the root's alone, so every depth
	 * counts the same; absent when none.
	 */
	ambiguous?: number;
	/**
	 * For a walk over callers, the files whose top level calls or uses the root's name and sees the root, in path order.
	 * `below_confidence` also counts the files the floor left out.
	 */
	file_scope?: FileScope[];
	/**
	 * For a walk over callers, how widely a change to the root reaches, from the callers in `reached`: `low`, `medium`,
	 * `high` or `critical`. A walk its `truncated` or `unexpanded` cut grades what it reached. Absent for a walk over
	 * callees.
	 */
	band?: string;
	/**
	 * The root's call of itself, which `reached` does not list, since the walk starts at the root and reaches each
	 * symbol once; absent when the root calls itself nowhere, or below the confidence floor, which `below_confidence`
	 * then counts it under.
	 */
	self_call?: SelfCall;
};

/**
 * A call of a symbol from its own body: a recursion, or a call of another definition of its name that the graph bound
 * to the symbol itself.
 */
export type SelfCall = {
	/** How sure the resolution of the call is, 15 to 95. */
	confidence: number;
	/**
	 * The one-based line of the call, in the symbol's own file; where the symbol calls itself from several lines, the
	 * first line among the calls the most confident tier bound.
	 */
	line: number;
};

/**
 * A file whose top level, outside every definition, calls or uses a name. The graph keeps only the names a top level
 * spells, so it has no line.
 */
export type FileScope = {
	file: string;
	/**
	 * How the file sees the symbol: 80 in the same file, 75 in its package, 70 through an include; 85 where the top
	 * level reads the name through the body that types the value, as Ruby's pasted bodies do. In `References` a
	 * member's file that sees it the first three ways is 15, a guess, as such a name keeps no receiver.
	 */
	confidence: number;
};

/** The params of `call_path`. */
export type CallPathParams = {
	from: string;
	/**
	 * Keeps the symbols `from` may select to the files whose root-relative path starts with it, one file's path or a
	 * directory's with its closing slash. Absent keeps every file.
	 */
	from_path_prefix?: string;
	/** Keeps the symbols `from` may select to those that start on this one-based line. Absent keeps every line. */
	from_line?: number;
	to: string;
	/**
	 * Keeps the symbols `to` may select to the files whose root-relative path starts with it, one file's path or a
	 * directory's with its closing slash. Absent keeps every file.
	 */
	to_path_prefix?: string;
	/** Keeps the symbols `to` may select to those that start on this one-based line. Absent keeps every line. */
	to_line?: number;
	lang?: string;
	/** How many calls long a path may be; 8 by default, and 0 follows no call. */
	depth?: number;
	min_confidence?: number;
	/** Caps the symbols the search reaches; 20,000 by default, and 0 sets no cap. */
	max_symbols?: number;
};

/** What a search for a chain of calls between two symbols found in one graph, and what it left out. */
export type CallPath = {
	graph: string;
	from: Definition;
	to: Definition;
	found: boolean;
	hops: Reached[];
	truncated: boolean;
	turned_away: number;
	/**
	 * The callers other than `from` the search reached at the depth bound, whose own callers it did not look at: a longer
	 * path may pass through them. Unlike `unexpanded`, it counts nothing the symbol cap left, which `truncated` reports.
	 */
	at_depth_bound: number;
	below_confidence: number;
	/** The other symbols each end's name selects. */
	passed_over: {from: number; to: number};
	/**
	 * The most called of each end's passed-over symbols, the most first, at most five each; `count` is each one's
	 * callers. Absent when neither end passed one over.
	 */
	candidates?: {from: Candidate[]; to: Candidate[]};
};

/** The params of `references`. */
export type ReferencesParams = {
	name: string;
	/**
	 * Keeps only the symbols in files whose root-relative path starts with it, one file's path or a directory's with
	 * its closing slash; it reaches one of several copies that share a qualified name. Absent looks in every file.
	 */
	path_prefix?: string;
	/**
	 * Keeps only the symbols that start on this one-based line, as a `Definition`'s `line`; it reaches one of several
	 * symbols of the name in one file. Absent looks at every line.
	 */
	line?: number;
	lang?: string;
	min_confidence?: number;
	/**
	 * Keeps the uses of one access and drops the calls: `read` also keeps the updates and the uses as a type, and `write`
	 * the updates.
	 */
	access?: Access;
	/** Caps the sites answered; 500 by default, and 0 answers the counts alone. */
	limit?: number;
};

/** How a site touches a symbol; a call reads it. */
export type Access = 'read' | 'write' | 'update' | 'type';

/** The sites that call or use a symbol in one graph. */
export type References = {
	graph: string;
	symbol: Definition;
	passed_over: number;
	/** The most used of the symbols passed over, the most first, at most five; `count` is each one's sites. */
	candidates?: Candidate[];
	sites: Site[];
	/** The sites that pass the confidence floor and the access kept, before the limit. */
	total: number;
	below_confidence: number;
	/** The sites that pass the confidence floor, by kind, whatever access is kept. */
	counts: SiteCounts;
	/**
	 * The calls of the symbol's name that bound no definition because several definitions of the name may be meant,
	 * and the uses of a member's name on a value whose type the graph could not follow: any of them may take this
	 * symbol.
	 */
	ambiguous: number;
	/**
	 * The files whose top level calls or uses the symbol's name and sees the symbol, in path order. They are no sites:
	 * the graph keeps no line for them and not how they touch the symbol, so neither `limit` nor `access` bounds them.
	 * `below_confidence` also counts the files the floor left out.
	 */
	file_scope?: FileScope[];
	/**
	 * Present, and `true`, when `symbol` is a type and its graph records no use of a type where code names it as one,
	 * such as a declaration, a signature, a generic argument or an import: `sites` then hold only the calls that name
	 * it and its reads as a value, so an empty `sites` does not show that nothing uses the type.
	 */
	type_uses_unrecorded?: boolean;
	/**
	 * Present, and `true`, when `symbol` is a field, a variable or a constant of a kind whose reads and writes its graph
	 * records nowhere: `sites` then hold only uses that spell the name as something else, so an empty `sites` does not
	 * show that nothing uses the symbol.
	 */
	data_uses_unrecorded?: boolean;
};

/** A symbol's sites, counted by kind. */
export type SiteCounts = {
	calls: number;
	reads: number;
	writes: number;
	updates: number;
	types: number;
};

/** A call or a use of a symbol, and the definition it sits in. */
export type Site = {
	site_of: Definition;
	line: number;
	confidence: number;
	/** A call rather than a use. */
	call: boolean;
	/** How the site touches the symbol: an `Access`, as `kind` is a `Definition`'s label. */
	access: string;
};

/** The params of `definitions`. */
export type DefinitionsParams = {
	name: string;
	lang?: string;
	/** The file the lookup asks from: definitions in its directory rank first. */
	from?: string;
	/** Caps the definitions answered; 10 by default, and 0 answers the total alone. */
	limit?: number;
};

/** The params of `sources`: those of `definitions`, and the caps on each definition's text. */
export type SourcesParams = DefinitionsParams & {
	/** Caps the lines of each definition, its doc comment included; 0 answers every line. */
	max_lines?: number;
	/** Caps the bytes of each line; 0 answers lines whole. */
	max_line_bytes?: number;
};

/** The answer of `definitions` and `sources` in one graph. */
export type Definitions = {
	graph: string;
	/** Every definition the name selects, before the limit. */
	total: number;
	found: Found[];
};

/** A definition a name selects. */
export type Found = {
	definition: Definition;
	/** `same_directory` or `elsewhere`, from the file the lookup asks from. */
	nearness: string;
	/** The definition's text, which only `sources` answers. */
	source?: Source;
	/** Why `sources` could not read the definition's text. */
	error?: string;
};

/** The text of a definition, its doc comment included, as its file holds it now. */
export type Source = {
	doc_line: number;
	first_line: number;
	lines: number;
	text: string;
	/** The lines of `text` the byte cap cut, each with its length before the cut. */
	cut_lines: Array<{line: number; bytes: number}>;
	/** The file's size differs from the size the build read, so the lines may have moved. */
	resized: boolean;
};

/** The params of `search`. */
export type SearchParams = {
	query: string;
	lang?: string;
	/** Caps the hits answered; 20 by default, and 0 answers the counts alone. */
	limit?: number;
};

/** The answer of `search` in one graph. */
export type Search = {
	graph: string;
	hits: Hit[];
	/** The definitions that share a word with the query or that it names, before the limit. */
	total: number;
	/** Those of `total` whose name matched, rather than only their file's path. */
	named: number;
	unmatched_words: string[];
	/** The distinct words of the query past the first eight, which were not looked for. */
	dropped_words: number;
	/** What the fill-up by characters found before the limit, when it ran. */
	fuzzy_total?: number;
};

/** A definition a search found. */
export type Hit = {
	definition: Definition;
	score: number;
	/** The name is the query as written. */
	exact: boolean;
	/** The fill-up by characters found it. */
	fuzzy: boolean;
};

/** The params of `implementations`. */
export type ImplementationsParams = {
	name: string;
	/**
	 * Keeps only the types in files whose root-relative path starts with it, one file's path or a directory's with its
	 * closing slash; it reaches one of several types that share a qualified name. Absent looks in every file.
	 */
	path_prefix?: string;
	/**
	 * Keeps only the types that start on this one-based line, as a `Definition`'s `line`; it reaches one of several
	 * types of the name in one file. Absent looks at every line.
	 */
	line?: number;
	lang?: string;
	/** How many levels below the root the search goes; 3 by default, and 0 finds none. */
	depth?: number;
	/** Caps the implementors answered; 100 by default, and 0 answers the total alone. */
	limit?: number;
};

/** The types that implement or extend a type in one graph. */
export type Implementations = {
	graph: string;
	root: Definition;
	passed_over: number;
	/**
	 * The most implemented of the types passed over, the most first, at most five; `count` is each one's direct
	 * implementors.
	 */
	candidates?: Candidate[];
	found: Array<{symbol: Definition; depth: number}>;
	/** The implementors within the depth bound, before the limit. */
	total: number;
	unexpanded: number;
};

/** The params of `dead_code`. */
export type DeadCodeParams = {
	lang?: string;
	/** The kinds to judge, by their labels; every kind when absent or empty. */
	kinds?: string[];
	/** Judges only the definitions in files under this path. */
	path_prefix?: string;
	/** Caps the definitions answered; 100 by default, and 0 answers the counts alone. */
	limit?: number;
};

/** The definitions of one graph nothing the graph sees uses, and the counts of what the judgement left out. */
export type DeadCode = {
	graph: string;
	found: Array<{definition: Definition; spelled: boolean}>;
	total: number;
	live: number;
	excluded: {
		kind: number;
		test: number;
		generated: number;
		entry: number;
		vendored: number;
		unobservable: number;
		uncertain: number;
		dispatched: number;
		reflected: number;
	};
};

/** The params of `resolution`. */
export type ResolutionParams = {
	lang?: string;
	/** How many unbound call sites of each graph to list; 20 by default, and 0 answers the counts alone. */
	limit?: number;
};

/** How the call sites of one graph bound. */
export type Resolution = {
	graph: string;
	sites: number;
	/** The sites bound at each confidence. */
	bound: Array<{confidence: number; sites: number}>;
	edges: number;
	misses: {
		external_import: number;
		not_exported: number;
		undefined: number;
		not_admitted: number;
		ambiguous: number;
		/** The sites the graph stopped reading at a bound, absent when none did. */
		past_bound?: number;
		through_a_receiver: number;
	};
	unbound: Unbound[];
	/** The unbound sites past the limit. */
	unlisted: number;
};

/** A call site that bound no symbol. */
export type Unbound = {
	caller: Definition;
	line: number;
	callee: string;
	receiver?: string;
	/** `external_import`, `not_exported`, `undefined`, `not_admitted`, `ambiguous` or `past_bound`. */
	miss: string;
};

/** The params of `outline`. */
export type OutlineParams = {
	file: string;
};

/**
 * What an answer about one file says of the file. `graph` is the graph of the language that claims the file, absent
 * when none does or when `missing` or `directory` is true; when that graph does not hold the file, `held` is false and
 * the answer holds these fields alone. `missing` is true when the index holds no such path, `directory` when it holds
 * the path as a directory, whose files are the ones to ask about, and `declined` names the language that leaves the
 * held file out of its graph; a held file with neither `graph` nor `declined` is one no language claims.
 */
export type FileState = {
	file: string;
	graph?: string;
	held: boolean;
	missing?: boolean;
	directory?: boolean;
	declined?: Declined;
};

/** The language that leaves a held file of its kind out of its graph, and the reason it states. */
export type Declined = {
	language: string;
	reason: string;
};

/** The definitions of one file, as {@link FileState} says; `symbols` is absent when `held` is false. */
export type Outline = FileState & {
	symbols?: Definition[];
};

/** The params of `dependents`. */
export type DependentsParams = {
	file: string;
	/** How many includes away the search goes; 3 by default, and 0 finds none. */
	depth?: number;
	/** Caps the files answered; 100 by default, and 0 sets no cap. */
	max_files?: number;
};

/**
 * The files that include a file, directly or through others, as {@link FileState} says; the fields past it are absent
 * when `held` is false.
 */
export type Dependents = FileState & {
	found?: Array<{file: string; depth: number}>;
	truncated?: boolean;
	turned_away?: number;
	unexpanded?: number;
};

/** The params of `analyze_segment`. */
export type SegmentParams = {
	file: string;
	/** The first line of the segment; absent takes the first line of the file. */
	start_line?: number;
	/** The last line of the segment; absent takes the end of the file. */
	end_line?: number;
	/** How many calls away each symbol's blast goes; 3 by default, and 0 follows no call. */
	depth?: number;
	min_confidence?: number;
	/** Caps the symbols each blast reaches; 500 by default, and 0 sets no cap. */
	max_symbols?: number;
};

/**
 * What a change to a range of a file's lines reaches, as {@link FileState} says; the fields past it are absent when
 * `held` is false.
 */
export type Segment = FileState & {
	start_line?: number;
	/** Where the params left it open, the last line a symbol of the file reaches, and never before `start_line`. */
	end_line?: number;
	symbols?: SegmentSymbol[];
	includes?: string[];
	included_by?: string[];
};

/** A symbol the segment overlaps, its direct calls each way, and the callers it reaches. */
export type SegmentSymbol = {
	definition: Definition;
	callers: DirectCalls;
	/**
	 * A field's, a variable's or a constant's direct users: the symbols that call it or whose code reads or writes it,
	 * which `blast` walks from as it walks from a function's callers; absent for any other kind, and for one with no
	 * user.
	 */
	users?: DirectCalls;
	/**
	 * The calls of its name a walk over its callers counts as {@link Walk}'s `ambiguous`: any of them may call it, and
	 * neither `callers` nor `blast` holds them; absent when none.
	 */
	ambiguous?: number;
	callees: DirectCalls;
	unresolved: number;
	blast: Blast;
};

/** A symbol's direct callers or callees: at most 50 listed, and `total` of them that pass the confidence floor. */
export type DirectCalls = {
	listed: Reached[];
	total: number;
	below_confidence: number;
};

/** What a walk over a symbol's callers reached. */
export type Blast = {
	/** The callers reached, the symbol itself not among them. */
	reach: number;
	/** The callers reached at each depth, the direct callers first. */
	by_depth: number[];
	/** The other files that hold a caller reached or include the symbol's file. */
	affected_files: number;
	/** `low`, `medium`, `high` or `critical`. */
	band: string;
	truncated: boolean;
	turned_away: number;
	unexpanded: number;
	below_confidence: number;
};

/** The params of `analyze_change`. */
export type ChangeParams = {
	regions: Region[];
	/** How many calls away each changed symbol's walk goes; 3 by default, and 0 follows no call. */
	depth?: number;
	min_confidence?: number;
	/** Caps the symbols each walk reaches; 500 by default, and 0 sets no cap. */
	max_symbols?: number;
};

/** A changed range of a file's lines; absent ends take the file's first and last lines. */
export type Region = {
	file: string;
	start_line?: number;
	end_line?: number;
};

/** What a change to regions across files reaches. */
export type Change = {
	changed_symbols: number;
	changed_files: number;
	unheld_regions: number;
	regions_outside_symbols: number;
	footprint: number;
	footprint_files: number;
	transitive_sum: number;
	hotspots: Hotspot[];
	total_hotspots: number;
	seeds: Seed[];
	truncated_seeds: number;
};

/** A symbol several changed symbols reach. */
export type Hotspot = {
	symbol: Definition;
	/** The changed symbols that reach it. */
	hit_count: number;
	/** How many calls away from the nearest of them it is. */
	min_depth: number;
	/** It is itself a changed symbol. */
	also_changed: boolean;
};

/** A changed symbol and the callers it reaches. */
export type Seed = {
	symbol: Definition;
	reach: number;
	/** `low`, `medium`, `high` or `critical`. */
	band: string;
	/** Its direct callers that pass the confidence floor. */
	caller_count: number;
	/**
	 * A field's, a variable's or a constant's direct users that pass the confidence floor: the symbols that call it or
	 * whose code reads or writes it, which the walk takes as a function's callers; absent when none, and for any other
	 * kind.
	 */
	user_count?: number;
	/**
	 * The calls of its name a walk over its callers counts as {@link Walk}'s `ambiguous`: any of them may call it, and
	 * neither `caller_count` nor `reach` counts them; absent when none.
	 */
	ambiguous?: number;
	/**
	 * Its direct users for a field, a variable or a constant, and its direct callers for any other kind, when there are
	 * 30 or fewer; empty otherwise.
	 */
	consumers: Reached[];
	/** Those direct users or callers by directory, the most first, when there are more than 30. */
	consumer_dirs: Array<{dir: string; count: number}>;
	truncated: boolean;
	turned_away: number;
	unexpanded: number;
	below_confidence: number;
};

/** The params of `list`. */
export type ListParams = {
	/** The directory to list; absent lists the root. */
	path?: string;
	/** How many levels to descend; absent lists the directory's children alone, and 0 its whole subtree. */
	depth?: number;
	/** Caps the entries answered; 1,000 by default, and 0 sets no cap. */
	limit?: number;
	/** Globs naming the directories listed without their entries. */
	collapse?: string[];
	/** Globs naming the entries left out. */
	exclude?: string[];
};

/** The answer of `list`. */
export type Listing = {
	path: string;
	entries: Listed[];
	truncated: boolean;
	/** The entries `exclude` left out. */
	excluded: number;
	/** The entries the index holds under the directory, at every depth. */
	total: number;
	/** Present when the answer reads a stopped build's index. */
	stopped_build?: StoppedBuild;
};

/** One entry of a listing. */
export type Listed = {
	path: string;
	/** `file`, `dir`, `symlink` or `other`. */
	kind: string;
	size?: number;
	binary?: boolean;
	/** Where a symlink points. */
	target?: string;
	/** The entries below a directory the listing did not descend into. */
	unlisted?: number;
};

/** The params of `glob`. */
export type GlobParams = {
	globs: string[];
	/** Globs naming the paths left out, a directory with everything beneath it. */
	exclude?: string[];
	/** Caps the paths answered; 0 sets no cap. */
	limit?: number;
};

/** The answer of `glob`: the matching paths in index order. */
export type Glob = {
	paths: string[];
	truncated: boolean;
	/** The matching paths `exclude` left out before the answer ended; absent when none. */
	excluded?: number;
	/** Present when the answer reads a stopped build's index. */
	stopped_build?: StoppedBuild;
};

/** The params of `fuzzy`. */
export type FuzzyParams = {
	query: string;
	/** Keep the paths they match; absent keeps every path. */
	globs?: string[];
	/** Globs naming the paths left out, a directory with everything beneath it. */
	exclude?: string[];
	/** Caps the matches answered; 50 by default, and 0 sets no cap. */
	limit?: number;
};

/** The answer of `fuzzy`: the best-ranked paths, best first. */
export type Fuzzy = {
	matches: Array<{path: string; score: number}>;
	truncated: boolean;
	/**
	 * The paths that matched the query, those past the limit included; absent for an empty query, which is not
	 * counted.
	 */
	matched?: number;
	/** Present when the answer reads a stopped build's index. */
	stopped_build?: StoppedBuild;
};

/** The params of `grep`. */
export type GrepParams = {
	/** Names the grep for a cancel; the handle passes it on as it is. */
	id?: number;
	/**
	 * Names the grep for a cancel `by` `binding_id`, apart from `id`: a binding over the wire names each call it runs
	 * by one, so the ids its callers give never stop a call of its own. The handle sets it, in place of any the params
	 * give.
	 */
	binding_id?: number;
	patterns: string[];
	case_insensitive?: boolean;
	/** Keep the files they match; absent keeps every file. */
	globs?: string[];
	/** Globs naming the files left out. */
	exclude?: string[];
	/** How many lines around each match to answer. */
	context?: number;
	/** Caps the matching lines answered; 100 by default, and 0 sets no cap. */
	limit?: number;
	/** Counts every matching line and file past the limit, into `total_lines` and `total_files`. */
	count_all?: boolean;
	/** Caps the bytes of each line answered; 1,024 by default, and 0 answers lines whole. */
	max_line_bytes?: number;
	/** The size past which a file is not searched; 4 MiB by default, and 0 searches every size. */
	max_file_bytes?: number;
};

/** The options of {@link Kartta.grep}: the params of `grep` but the binding id the handle gives each grep. */
export type GrepOptions = Omit<GrepParams, 'binding_id'>;

/** The answer of `grep`: the files holding a match with their lines, and the account of every candidate. */
export type Grep = {
	files: Array<{path: string; lines: GrepLine[]}>;
	/** The matching lines answered. */
	lines: number;
	truncated: boolean;
	total_lines?: number;
	total_files?: number;
	cancelled: boolean;
	candidates: number;
	excluded: number;
	symlinks: number;
	searched: number;
	binary: number;
	oversized: number;
	/** The candidates the content index ruled out unread. */
	ruled_out: number;
	unsearched: number;
	failures: Array<{path: string; message: string}>;
	/** Present when the answer reads a stopped build's index. */
	stopped_build?: StoppedBuild;
};

/** A matching line, or a line of context around one. */
export type GrepLine = {
	number: number;
	/** `match` or `context`. */
	kind: string;
	text: string;
	/** The bytes left out before and after the part of the line answered, when `max_line_bytes` cut it. */
	cut?: {before: number; after: number};
};
