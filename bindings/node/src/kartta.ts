/**
 * Node.js binding of the kartta code graph engine: a handle on a tree, its build, and every query the engine answers.
 *
 * @module
 */

import {createRequire} from 'node:module';

import {target} from './target.ts';
import type {
	BuildOptions,
	CallPath,
	CallPathParams,
	CancelAnswer,
	CancelParams,
	Change,
	ChangeParams,
	Code,
	DeadCode,
	DeadCodeParams,
	Definitions,
	DefinitionsParams,
	Dependents,
	DependentsParams,
	Fuzzy,
	FuzzyParams,
	Glob,
	GlobParams,
	Grep,
	GrepParams,
	Implementations,
	ImplementationsParams,
	ListParams,
	Listing,
	Matches,
	Methods,
	Outline,
	OutlineParams,
	References,
	ReferencesParams,
	Resolution,
	ResolutionParams,
	Search,
	SearchParams,
	Segment,
	SegmentParams,
	SourcesParams,
	Stats,
	Version,
	Walk,
	WalkParams,
} from './wire.ts';

export type * from './wire.ts';

const TARGETS = ['darwin-arm64', 'linux-x64-gnu', 'linux-arm64-gnu'];

type Addon = {
	open(params: string): Handle | Buffer;
};

type Handle = {
	build(params: string): Promise<Buffer>;
	call(method: string, params: string): Promise<Buffer>;
	cancel(params: string): Buffer;
	close(): void;
};

/** A failure the engine answered, or a call on a closed handle: a code to branch on, and a message for a reader. */
export class KarttaError extends Error {
	override name = 'KarttaError';
	readonly code: Code;

	constructor(code: Code, message: string, options?: ErrorOptions) {
		super(message, options);
		this.code = code;
	}
}

/**
 * A handle on one tree. {@link Kartta.open} names the tree; {@link Kartta.build} walks it once and builds the file
 * index and every graph; each query asks what the last finished build made.
 *
 * Every method but `open` and `close` answers a promise and runs off the event loop: a build on a thread of its own,
 * a query on libuv's thread pool. Queries run concurrently with each other; builds run one at a time, and while one
 * walks, a query rejects with `building`. A failure rejects with a {@link KarttaError} carrying the wire's code.
 */
export class Kartta {
	#handle: Handle | undefined;
	#builds = 0;
	readonly #inFlight = new Set<number>();

	private constructor(handle: Handle) {
		this.#handle = handle;
	}

	/**
	 * Opens a handle on the tree at `root`. Nothing is walked until {@link Kartta.build}.
	 *
	 * @throws {KarttaError} `invalid_root` when `root` is not a directory this process can read the metadata of.
	 * @throws {Error} When this package holds no addon for the platform it runs on.
	 */
	static open(root: string): Kartta {
		const opened = addon().open(JSON.stringify({root}));
		if (Buffer.isBuffer(opened)) {
			// SAFETY: the addon answers a failed open as the wire's error object, which the golden files pin.
			const {code, message} = JSON.parse(opened.toString()) as {code: Code; message: string};
			throw new KarttaError(code, message);
		}
		return new Kartta(opened);
	}

	/**
	 * Walks the tree and builds the file index and the graphs `options` name, replacing what an earlier build made.
	 *
	 * Once `signal` aborts, a build still running stops and rejects with `build_cancelled`, the signal's reason its
	 * cause; one that finished first answers its stats, and the handle holds what it built. A build that fails or stops
	 * leaves the handle without an index until the next one finishes. A signal aborted before the call rejects with its
	 * reason alone and leaves the handle as it was.
	 */
	build(options: BuildOptions = {}, {signal}: {signal?: AbortSignal} = {}): Promise<Stats> {
		// SAFETY: the dispatch answers `build` with the wire's stats, which the golden files pin.
		return this.#build(options, signal) as Promise<Stats>;
	}

	/**
	 * Stops the build or the grep `params` names by its id, or, when it names neither, every build in flight, running or
	 * waiting its turn, and every grep in flight. A build {@link Kartta.build} started rejects with `build_cancelled`; a
	 * grep answers what it covered, `cancelled` set.
	 */
	cancel(params: CancelParams = {}): Promise<CancelAnswer> {
		return this.#ask('cancel', params);
	}

	async #build(params: Record<string, unknown>, signal?: AbortSignal): Promise<unknown> {
		const handle = this.#open();
		signal?.throwIfAborted();
		this.#builds += 1;
		const id = this.#builds;
		const cancel = (): void => {
			this.#handle?.cancel(JSON.stringify({build: id}));
		};
		this.#inFlight.add(id);
		signal?.addEventListener('abort', cancel, {once: true});
		try {
			return answer(await handle.build(JSON.stringify({...params, id})));
		} catch (error) {
			if (!(error instanceof KarttaError) || error.code !== 'build_cancelled') {
				throw error;
			}
			if (signal?.aborted === true) {
				throw new KarttaError(error.code, error.message, {cause: signal.reason});
			}
			if (this.#handle === undefined) {
				throw new KarttaError(error.code, error.message, {cause: closed()});
			}
			throw error;
		} finally {
			signal?.removeEventListener('abort', cancel);
			this.#inFlight.delete(id);
		}
	}

	/** Answers what the last build indexed, what it made of each language's files, and what each graph holds. */
	stats(): Promise<Stats> {
		return this.#ask('stats', {});
	}

	/** Answers the library's ABI version, its crate version, the methods it answers and the languages it ships. */
	version(): Promise<Version> {
		return this.#ask('version', {});
	}

	/** Answers how the call sites of each graph bound, and lists those that bound nothing. */
	resolution(params: ResolutionParams = {}): Promise<Matches<Resolution>> {
		return this.#ask('resolution', params);
	}

	/** Answers the callers of the symbol `params` names, transitively, in each graph that holds one. */
	callers(params: WalkParams): Promise<Matches<Walk>> {
		return this.#ask('callers', params);
	}

	/** Answers the callees of the symbol `params` names, transitively, in each graph that holds one. */
	callees(params: WalkParams): Promise<Matches<Walk>> {
		return this.#ask('callees', params);
	}

	/** Answers a chain of calls from one symbol to another, in each graph that holds both. */
	callPath(params: CallPathParams): Promise<Matches<CallPath>> {
		return this.#ask('call_path', params);
	}

	/** Answers the sites that call or use the symbol `params` names, in each graph that holds one. */
	references(params: ReferencesParams): Promise<Matches<References>> {
		return this.#ask('references', params);
	}

	/** Answers the definitions the name `params` gives selects, in each graph that holds one. */
	definitions(params: DefinitionsParams): Promise<Matches<Definitions>> {
		return this.#ask('definitions', params);
	}

	/** Answers the definitions the name `params` gives selects, each with its text as its file holds it now. */
	sources(params: SourcesParams): Promise<Matches<Definitions>> {
		return this.#ask('sources', params);
	}

	/** Answers the definitions that best match the words of a query, in each graph. */
	search(params: SearchParams): Promise<Matches<Search>> {
		return this.#ask('search', params);
	}

	/** Answers the types that implement or extend the type `params` names, in each graph that holds one. */
	implementations(params: ImplementationsParams): Promise<Matches<Implementations>> {
		return this.#ask('implementations', params);
	}

	/** Answers the definitions of each graph that nothing the graph sees uses. */
	deadCode(params: DeadCodeParams = {}): Promise<Matches<DeadCode>> {
		return this.#ask('dead_code', params);
	}

	/** Answers the definitions of one file. */
	outline(params: OutlineParams): Promise<Outline> {
		return this.#ask('outline', params);
	}

	/** Answers the files that include a file, directly or through others. */
	dependents(params: DependentsParams): Promise<Dependents> {
		return this.#ask('dependents', params);
	}

	/** Answers what a change to a range of a file's lines reaches. */
	analyzeSegment(params: SegmentParams): Promise<Segment> {
		return this.#ask('analyze_segment', params);
	}

	/** Answers what a change to regions across files reaches. */
	analyzeChange(params: ChangeParams): Promise<Change> {
		return this.#ask('analyze_change', params);
	}

	/**
	 * Answers the entries under a directory of the index. A path the index does not hold rejects with `not_found`, and
	 * one that is not a directory with `not_a_directory`.
	 */
	list(params: ListParams = {}): Promise<Listing> {
		return this.#ask('list', params);
	}

	/** Answers the paths of the index the globs match. A glob that does not compile rejects with `invalid_glob`. */
	glob(params: GlobParams): Promise<Glob> {
		return this.#ask('glob', params);
	}

	/** Answers the paths of the index that best match a query. */
	fuzzy(params: FuzzyParams): Promise<Fuzzy> {
		return this.#ask('fuzzy', params);
	}

	/**
	 * Answers the lines of the indexed files the patterns match. A pattern that does not compile rejects with
	 * `invalid_pattern`, and a glob with `invalid_glob`.
	 */
	grep(params: GrepParams): Promise<Grep> {
		return this.#ask('grep', params);
	}

	/**
	 * Answers `method` with `params`, as the wire spells both, for a method this class has no typed call for or params
	 * its types do not hold. `build` runs as {@link Kartta.build} does, under an id the handle picks in place of one its
	 * params give, so no two builds share one; `cancel` answers at once.
	 */
	async call(method: string, params: unknown = {}): Promise<unknown> {
		if (method === 'build' && typeof params === 'object' && !Array.isArray(params)) {
			return this.#build({...params});
		}
		const handle = this.#open();
		const text = JSON.stringify(params);
		switch (method) {
			case 'build':
				return answer(await handle.build(text));
			case 'cancel':
				return answer(handle.cancel(text));
			default:
				return answer(await handle.call(method, text));
		}
	}

	/**
	 * Cancels the builds and greps in flight and lets the engine go once the calls in flight answer. A build this cancels
	 * rejects with `build_cancelled`, a `closed` error its cause; of the builds that have yet to enter the library, only
	 * the last 1,024 it cancels stop. Every later call rejects with `closed`. Closing a closed handle does nothing.
	 */
	close(): void {
		const handle = this.#handle;
		if (handle === undefined) {
			return;
		}
		this.#handle = undefined;
		for (const id of this.#inFlight) {
			handle.cancel(JSON.stringify({build: id}));
		}
		handle.cancel('{}');
		handle.close();
	}

	#open(): Handle {
		if (this.#handle === undefined) {
			throw closed();
		}
		return this.#handle;
	}

	#ask<M extends keyof Methods>(method: M, params: Methods[M]['params']): Promise<Methods[M]['answer']> {
		// SAFETY: the dispatch answers each method with the wire's answer `Methods` names, which the golden files pin.
		return this.call(method, params) as Promise<Methods[M]['answer']>;
	}
}

let loaded: Addon | undefined;

function addon(): Addon {
	loaded ??= load();
	return loaded;
}

function load(): Addon {
	const host = target();
	const require = createRequire(import.meta.url);
	for (const id of [`../kartta.${host}.node`, `kartta-${host}`]) {
		try {
			// SAFETY: both ids name the addon crates/kartta-node builds, whose surface `Addon` declares.
			return require(id) as Addon;
		} catch (error) {
			if (!(error instanceof Error && 'code' in error && error.code === 'MODULE_NOT_FOUND')) {
				throw error;
			}
		}
	}
	throw new Error(
		`kartta holds no addon for ${host}; it ships one for ${TARGETS.join(', ')}, ` +
			'and `npm run build` in a checkout builds one for the host',
	);
}

function answer(envelope: Buffer): unknown {
	// SAFETY: the dispatch answers every call with the wire's envelope, `{ok}` or `{err: {code, message}}`.
	const parsed = JSON.parse(envelope.toString()) as {ok: unknown} | {err: {code: Code; message: string}};
	if ('err' in parsed) {
		throw new KarttaError(parsed.err.code, parsed.err.message);
	}
	return parsed.ok;
}

function closed(): KarttaError {
	return new KarttaError('closed', 'the handle is closed; open another');
}
