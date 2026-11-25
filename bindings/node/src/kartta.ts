/**
 * Node.js binding of the kartta code graph engine: a handle on a tree, its build, and every query the engine answers.
 *
 * @module
 */

import {createRequire} from 'node:module';

import {TARGETS, target} from './target.ts';
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
	GrepOptions,
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
	Refresh,
	RefreshOptions,
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

const decoder = new TextDecoder();

type Addon = {
	open(params: string): Handle | Uint8Array;
};

type Handle = {
	build(params: string): Promise<Uint8Array>;
	refresh(params: string): Promise<Uint8Array>;
	call(method: string, params: string): Promise<Uint8Array>;
	cancel(params: string): Uint8Array;
	close(): void;
};

/**
 * The methods that run under an id a cancel names by the method: a build and a refresh each on a thread of its own, a
 * grep on libuv's pool.
 */
type Tracked = 'build' | 'grep' | 'refresh';

/**
 * A failure the engine answered, a call on a closed handle, or `internal` for a failure of the addon or of reading its
 * answer, the original error its `cause`: a code to branch on, and a message for a reader.
 */
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
 * Every method but `open` and `close` answers a promise and runs off the event loop: a build and a refresh each on a
 * thread of its own, a query on libuv's thread pool. Queries run concurrently with each other; builds run one at a
 * time, and while one walks, a query rejects with `building`; a query waits for a refresh under way. A failure rejects
 * with a {@link KarttaError}.
 */
export class Kartta {
	#handle: Handle | undefined;
	#ids = 0;
	readonly #inFlight = new Map<number, Tracked>();

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
		if (opened instanceof Uint8Array) {
			// SAFETY: the addon answers a failed open as the wire's error object, which the golden files pin.
			// oxlint-disable-next-line typescript/no-unsafe-type-assertion
			const {code, message} = JSON.parse(decoder.decode(opened)) as {code: Code; message: string};
			throw new KarttaError(code, message);
		}
		return new Kartta(opened);
	}

	/**
	 * Walks the tree and builds the file index and the graphs `options` name, replacing what an earlier build made and
	 * ending its watch. With `watch` set, the index follows the tree until the next build or `close`.
	 *
	 * Once `signal` aborts, a build still running stops and rejects with `build_cancelled`, the signal's reason its
	 * cause; one that finished first answers its stats, and the handle holds what it built. A cancel past the last file
	 * rejects the same way, and the handle holds the whole engine. A build that fails leaves the handle without an index
	 * until the next one finishes. One that stops leaves the index of what it reached, and `stats` and the file queries
	 * answer from it, each naming it in `stopped_build`, until the next one begins; one that reached no entry leaves
	 * nothing, so they reject with `not_built`. A signal aborted before the call
	 * rejects with its reason alone and leaves the handle as it was.
	 */
	async build(options: BuildOptions = {}, {signal}: {signal?: AbortSignal} = {}): Promise<Stats> {
		// SAFETY: the dispatch answers `build` with the wire's stats, which the golden files pin.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		return this.#tracked('build', options, signal) as Promise<Stats>;
	}

	/**
	 * Brings the index and the graphs up to the tree in place, walking again the paths `options` name, or the whole tree
	 * without them, and answers what changed. Every query waits for a refresh under way. A handle built without
	 * `stamps` rejects with `no_stamps`; a refresh that fails leaves the index as it was.
	 *
	 * Once `signal` aborts, a refresh still walking stops and rejects with `refresh_cancelled`, the signal's reason its
	 * cause, and leaves the index as it was; one past its walk answers what it changed. A signal aborted before the call
	 * rejects with its reason alone.
	 */
	async refresh(options: RefreshOptions = {}, {signal}: {signal?: AbortSignal} = {}): Promise<Refresh> {
		// SAFETY: the dispatch answers `refresh` with the wire's refresh, which the golden files pin.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		return this.#tracked('refresh', options, signal) as Promise<Refresh>;
	}

	/**
	 * Stops the build, the grep or the refresh `params` names by its id, or by its binding id when `by` says so, or, when
	 * it names none, every build in flight, running or waiting its turn, and every grep and refresh in flight. A build
	 * {@link Kartta.build} started rejects with `build_cancelled`; a grep answers what it covered, `cancelled` set; a
	 * refresh still walking rejects with `refresh_cancelled`.
	 */
	async cancel(params: CancelParams = {}): Promise<CancelAnswer> {
		return this.#ask('cancel', params);
	}

	async #tracked(method: Tracked, params: Record<string, unknown>, signal?: AbortSignal): Promise<unknown> {
		const handle = this.#open();
		signal?.throwIfAborted();
		this.#ids += 1;
		const id = this.#ids;
		const cancel = (): void => {
			this.#handle?.cancel(JSON.stringify({[method]: id, by: 'binding_id'}));
		};
		this.#inFlight.set(id, method);
		signal?.addEventListener('abort', cancel, {once: true});
		try {
			const text = JSON.stringify({...params, binding_id: id});
			return await answered(async () => (method === 'grep' ? handle.call(method, text) : handle[method](text)));
		} catch (error) {
			if (!(error instanceof KarttaError) || error.code !== `${method}_cancelled`) {
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
	async stats(): Promise<Stats> {
		return this.#ask('stats', {});
	}

	/** Answers the library's ABI version, its crate version, the methods it answers and the languages it ships. */
	async version(): Promise<Version> {
		return this.#ask('version', {});
	}

	/** Answers how the call sites of each graph bound, and lists those that bound nothing. */
	async resolution(params: ResolutionParams): Promise<Matches<Resolution>> {
		return this.#ask('resolution', params);
	}

	/** Answers the callers of the symbol `params` names, transitively, in each graph that holds one. */
	async callers(params: WalkParams): Promise<Matches<Walk>> {
		return this.#ask('callers', params);
	}

	/** Answers the callees of the symbol `params` names, transitively, in each graph that holds one. */
	async callees(params: WalkParams): Promise<Matches<Walk>> {
		return this.#ask('callees', params);
	}

	/** Answers a chain of calls from one symbol to another, in each graph that holds both. */
	async callPath(params: CallPathParams): Promise<Matches<CallPath>> {
		return this.#ask('call_path', params);
	}

	/** Answers the sites that call or use the symbol `params` names, in each graph that holds one. */
	async references(params: ReferencesParams): Promise<Matches<References>> {
		return this.#ask('references', params);
	}

	/** Answers the definitions the name `params` gives selects, in each graph that holds one. */
	async definitions(params: DefinitionsParams): Promise<Matches<Definitions>> {
		return this.#ask('definitions', params);
	}

	/** Answers the definitions the name `params` gives selects, each with its text as its file holds it now. */
	async sources(params: SourcesParams): Promise<Matches<Definitions>> {
		return this.#ask('sources', params);
	}

	/** Answers the definitions that best match the words of a query, in each graph. */
	async search(params: SearchParams): Promise<Matches<Search>> {
		return this.#ask('search', params);
	}

	/** Answers the types that implement or extend the type `params` names, in each graph that holds one. */
	async implementations(params: ImplementationsParams): Promise<Matches<Implementations>> {
		return this.#ask('implementations', params);
	}

	/** Answers the definitions of each graph that nothing the graph sees uses. */
	async deadCode(params: DeadCodeParams): Promise<Matches<DeadCode>> {
		return this.#ask('dead_code', params);
	}

	/** Answers the definitions of one file. */
	async outline(params: OutlineParams): Promise<Outline> {
		return this.#ask('outline', params);
	}

	/** Answers the files that include a file, directly or through others. */
	async dependents(params: DependentsParams): Promise<Dependents> {
		return this.#ask('dependents', params);
	}

	/** Answers what a change to a range of a file's lines reaches. */
	async analyzeSegment(params: SegmentParams): Promise<Segment> {
		return this.#ask('analyze_segment', params);
	}

	/** Answers what a change to regions across files reaches. */
	async analyzeChange(params: ChangeParams): Promise<Change> {
		return this.#ask('analyze_change', params);
	}

	/**
	 * Answers the entries under a directory of the index. A path the index does not hold rejects with `not_found`, and
	 * one that is not a directory with `not_a_directory`.
	 */
	async list(params: ListParams = {}): Promise<Listing> {
		return this.#ask('list', params);
	}

	/** Answers the paths of the index the globs match. A glob that does not compile rejects with `invalid_glob`. */
	async glob(params: GlobParams): Promise<Glob> {
		return this.#ask('glob', params);
	}

	/** Answers the paths of the index that best match a query. */
	async fuzzy(params: FuzzyParams): Promise<Fuzzy> {
		return this.#ask('fuzzy', params);
	}

	/**
	 * Answers the lines of the indexed files the patterns match. A pattern that does not compile rejects with
	 * `invalid_pattern`, and a glob with `invalid_glob`.
	 *
	 * Once `signal` aborts, a grep that has not opened its last file, waiting for a pool thread or searching, stops and
	 * answers what it covered, `cancelled` set and the files it did not open counted `unsearched`; one that has answers
	 * whole. A signal aborted before the call rejects with its reason alone.
	 */
	async grep(options: GrepOptions, {signal}: {signal?: AbortSignal} = {}): Promise<Grep> {
		// SAFETY: the dispatch answers `grep` with the wire's grep, which the golden files pin.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		return this.#tracked('grep', options, signal) as Promise<Grep>;
	}

	/**
	 * Answers `method` with `params`, as the wire spells both, for a method this class has no typed call for or params
	 * its types do not hold. `build`, `grep` and `refresh` run as their typed calls do, under a binding id the handle
	 * picks in place of any their params give, and the id their params give passes on as it is: a cancel naming it stops
	 * the calls given that id and none the handle named alone. `cancel` answers at once.
	 */
	async call(method: string, params: unknown = {}): Promise<unknown> {
		if (
			(method === 'build' || method === 'grep' || method === 'refresh') &&
			typeof params === 'object' &&
			!Array.isArray(params)
		) {
			return this.#tracked(method, {...params});
		}
		const handle = this.#open();
		const text = JSON.stringify(params);
		return answered(async () => {
			switch (method) {
				case 'build': {
					return handle.build(text);
				}
				case 'refresh': {
					return handle.refresh(text);
				}
				case 'cancel': {
					return handle.cancel(text);
				}
				default: {
					return handle.call(method, text);
				}
			}
		});
	}

	/**
	 * Cancels the builds, greps and refreshes in flight and lets the engine go once the calls in flight answer. A build
	 * this cancels rejects with `build_cancelled` and a refresh with `refresh_cancelled`, a `closed` error the cause of
	 * each, and a grep answers what it covered, `cancelled` set; of the builds, the greps and the refreshes that have yet
	 * to enter the library, only the last 1,024 of each it cancels stop. The watch of a build that asked for one ends
	 * within about a second of the engine going, and lets its watcher go. Every later call rejects with `closed`.
	 * Closing a closed handle does nothing.
	 */
	close(): void {
		const handle = this.#handle;
		if (handle === undefined) {
			return;
		}
		this.#handle = undefined;
		for (const [id, method] of this.#inFlight) {
			handle.cancel(JSON.stringify({[method]: id, by: 'binding_id'}));
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

	async #ask<M extends keyof Methods>(method: M, params: Methods[M]['params']): Promise<Methods[M]['answer']> {
		// SAFETY: the dispatch answers each method with the wire's answer `Methods` names, which the golden files pin.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
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
			// oxlint-disable-next-line typescript/no-unsafe-type-assertion
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

/** What `call` answers through the addon, a failure of the addon or of reading the answer rejecting with `internal`. */
async function answered(call: () => Promise<Uint8Array>): Promise<unknown> {
	try {
		return answer(await call());
	} catch (error) {
		if (error instanceof KarttaError) {
			throw error;
		}
		throw new KarttaError('internal', error instanceof Error ? error.message : String(error), {cause: error});
	}
}

function answer(envelope: Uint8Array): unknown {
	// SAFETY: the dispatch answers every call with the wire's envelope, `{ok}` or `{err: {code, message}}`.
	// oxlint-disable-next-line typescript/no-unsafe-type-assertion
	const parsed = JSON.parse(decoder.decode(envelope)) as {ok: unknown} | {err: {code: Code; message: string}};
	if ('err' in parsed) {
		throw new KarttaError(parsed.err.code, parsed.err.message);
	}
	return parsed.ok;
}

function closed(): KarttaError {
	return new KarttaError('closed', 'the handle is closed; open another');
}
