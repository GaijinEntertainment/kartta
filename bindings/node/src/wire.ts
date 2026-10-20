/**
 * The wire `kartta-ffi` answers: each method's params and answer, field for field as the JSON spells them. A bound
 * left out or 0 takes the method's default, except where a field says 0 sets no cap.
 *
 * @module
 */

/** Each method of the wire, its params and its answer. */
export type Methods = {
	build: {params: BuildParams; answer: Stats};
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
 * closed handle.
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
	/** Names the build for a cancel; {@link Kartta.build} sets it. */
	id?: number;
	/** The languages to build graphs of, by the names `version` lists; absent builds every one, empty none. */
	languages?: string[];
	/** Bounds the threads the index runs on; absent takes every core. */
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
};

/** The options of {@link Kartta.build}: the params of `build` but the id the binding gives each build. */
export type BuildOptions = Omit<BuildParams, 'id'>;

/** Extends a built-in set, or replaces it. */
export type SetChange = {extend: string[]} | {replace: string[]};

/** Where a content index lives: in memory, or in scratch files the file and mapped storages create in `dir`. */
export type ContentIndex = {storage: 'memory'} | {storage: 'file' | 'mapped'; dir: string};

/**
 * The params of `cancel`. Naming neither a build nor a grep stops every build in flight, running or waiting its turn,
 * and every grep in flight.
 */
export type CancelParams = {
	/** The id of the build to stop. */
	build?: number;
	/** The id of the grep to stop. */
	grep?: number;
};

/** The answer of `cancel`. */
export type CancelAnswer = {
	/**
	 * Whether a build or a grep in flight was stopped. A named build or grep that has not entered the library starts
	 * stopped, unless cancels that stopped nothing named 1,024 distinct later ids of its kind before it entered; a grep
	 * that had already opened its last file answers whole, with `cancelled` false.
	 */
	stopped: boolean;
};

/** The answer of `build` and `stats`. */
export type Stats = {
	/** The entries the index holds, directories included. */
	entries: number;
	languages: LanguageFiles[];
	graphs: GraphStats[];
	/** Absent when the build made no content index. */
	content_index?: ContentIndexStats;
	errors: BuildErrors;
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
	uses: number;
	includes: number;
};

/** What a content index holds. */
export type ContentIndexStats = {
	files: number;
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
	/** `walk`, `ignore`, `name`, `read`, `content_index`, `cache` or `other`. */
	kind: string;
	path?: string;
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
	/** Names the graph to ask, by a language or its family; absent asks every graph. */
	lang?: string;
	/** How many calls away the walk goes; callers default to 3, callees to 1. */
	depth?: number;
	/** The least confidence, 0 to 100, a call needs to be taken. */
	min_confidence?: number;
	/** Caps the symbols the walk reaches, its root included; 500 by default. */
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
	/** The symbols at the depth bound whose own calls were not followed. */
	unexpanded: number;
	/** The calls under the confidence floor. */
	below_confidence: number;
	/** The calls that bound no symbol the graph holds. */
	unresolved: number;
	/**
	 * For a walk over callers, the files whose top level calls or uses the root's name and sees the root, in path order.
	 * `below_confidence` also counts the files the floor left out.
	 */
	file_scope?: FileScope[];
};

/**
 * A file whose top level, outside every definition, calls or uses a name. The graph keeps only the names a top level
 * spells, so it has no line.
 */
export type FileScope = {
	file: string;
	/** How the file sees the symbol: 80 in the same file, 75 in its package, 70 through an include. */
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
	to: string;
	/**
	 * Keeps the symbols `to` may select to the files whose root-relative path starts with it, one file's path or a
	 * directory's with its closing slash. Absent keeps every file.
	 */
	to_path_prefix?: string;
	lang?: string;
	/** How many calls long a path may be; 8 by default. */
	depth?: number;
	min_confidence?: number;
	/** Caps the symbols the search reaches; 20,000 by default. */
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
	/** The symbols at the depth bound whose own calls were not followed. */
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
	lang?: string;
	min_confidence?: number;
	/** Keeps the uses that read, set or update the symbol, an update counting as both, and drops the calls. */
	access?: Access;
	/** Caps the sites answered; 500 by default. */
	limit?: number;
};

/** How a site touches a symbol; a call, and a use as a type, read it. */
export type Access = 'read' | 'write' | 'update';

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
	/** The uses of a member's name on a value whose type the graph could not follow, which may take this symbol. */
	ambiguous: number;
};

/** A symbol's sites, counted by kind. */
export type SiteCounts = {
	calls: number;
	reads: number;
	writes: number;
	updates: number;
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
	/** Caps the definitions answered; 10 by default. */
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
	/** Caps the hits answered; 20 by default. */
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
	lang?: string;
	/** How many levels below the root the search goes; 3 by default. */
	depth?: number;
	/** Caps the implementors answered; 100 by default. */
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
	/** Caps the definitions answered; 100 by default. */
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
	};
};

/** The params of `resolution`. */
export type ResolutionParams = {
	lang?: string;
	/** How many unbound call sites of each graph to list; 20 by default. */
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
	/** `external_import`, `not_exported`, `undefined`, `not_admitted` or `ambiguous`. */
	miss: string;
};

/** The params of `outline`. */
export type OutlineParams = {
	file: string;
};

/**
 * The definitions of one file. `graph` is the graph of the language that claims the file, absent when none does; when
 * that graph does not hold the file, `held` is false and the rest is empty.
 */
export type Outline = {
	file: string;
	graph?: string;
	held: boolean;
	symbols: Definition[];
};

/** The params of `dependents`. */
export type DependentsParams = {
	file: string;
	/** How many includes away the search goes; 3 by default. */
	depth?: number;
	/** Caps the files answered; 100 by default, and 0 sets no cap. */
	max_files?: number;
};

/** The files that include a file, directly or through others. `graph` and `held` are as {@link Outline}'s. */
export type Dependents = {
	file: string;
	graph?: string;
	held: boolean;
	found: Array<{file: string; depth: number}>;
	truncated: boolean;
	turned_away: number;
	unexpanded: number;
};

/** The params of `analyze_segment`. */
export type SegmentParams = {
	file: string;
	/** The first line of the segment; absent takes the first line of the file. */
	start_line?: number;
	/** The last line of the segment; absent takes the end of the file. */
	end_line?: number;
	/** How many calls away each symbol's blast goes; 3 by default. */
	depth?: number;
	min_confidence?: number;
	/** Caps the symbols each blast reaches; 500 by default. */
	max_symbols?: number;
};

/** What a change to a range of a file's lines reaches. `graph` and `held` are as {@link Outline}'s. */
export type Segment = {
	file: string;
	graph?: string;
	held: boolean;
	start_line: number;
	end_line: number;
	symbols: SegmentSymbol[];
	includes: string[];
	included_by: string[];
};

/** A symbol the segment overlaps, its direct calls each way, and the callers it reaches. */
export type SegmentSymbol = {
	definition: Definition;
	callers: DirectCalls;
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
	/** How many calls away each changed symbol's walk goes; 3 by default. */
	depth?: number;
	min_confidence?: number;
	/** Caps the symbols each walk reaches; 500 by default. */
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
	/** Those direct callers when there are 30 or fewer; empty otherwise. */
	consumers: Reached[];
	/** Those direct callers by directory, the most first, when there are more than 30. */
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
};

/** The params of `fuzzy`. */
export type FuzzyParams = {
	query: string;
	/** Keep the paths they match; absent keeps every path. */
	globs?: string[];
	/** Globs naming the paths left out, a directory with everything beneath it. */
	exclude?: string[];
	/** Caps the matches answered; 50 by default. */
	limit?: number;
};

/** The answer of `fuzzy`: the best-ranked paths, best first. */
export type Fuzzy = {
	matches: Array<{path: string; score: number}>;
	truncated: boolean;
};

/** The params of `grep`. */
export type GrepParams = {
	/** Names the grep for a cancel. */
	id?: number;
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
	/** Caps the bytes of each line answered; 1,024 by default. */
	max_line_bytes?: number;
	/** The size past which a file is not searched; 4 MiB by default. */
	max_file_bytes?: number;
};

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
