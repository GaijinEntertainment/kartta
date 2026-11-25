import {execFileSync} from 'node:child_process';
import {pbkdf2} from 'node:crypto';
import {mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {setImmediate} from 'node:timers/promises';
import {promisify} from 'node:util';

import {afterAll, beforeAll, beforeEach, expect, onTestFailed, onTestFinished, test, vi} from 'vite-plus/test';

import {Kartta, KarttaError, type Stats} from '../src/kartta.ts';

/**
 * The steps the running test reached, each at the milliseconds since it started, printed when it fails. A step that
 * holds the JS thread shows as the gap before the step after it, where a timeout names only the test.
 */
const reached: string[] = [];
let started = 0;

function step(name: string): void {
	reached.push(`${name} +${(performance.now() - started).toFixed(1)} ms`);
}

beforeEach(() => {
	started = performance.now();
	reached.length = 0;
	onTestFailed(() => {
		console.error(`the steps the test reached: ${reached.join(', ')}`);
	});
});

/** Writes a new tree of `files` Go files, each calling the next, and answers its root. */
function written(files: number): string {
	const root = mkdtempSync(path.join(tmpdir(), 'kartta-node-'));
	for (let file = 0; file < files; file += 1) {
		writeFileSync(path.join(root, `f${file}.go`), `package main\n\nfunc f${file}() {\n\tf${(file + 1) % files}()\n}\n`);
	}
	return root;
}

/** A tree of `files` files that the test removes once it finishes. */
function tree(files: number): string {
	step(`writing ${files} files`);
	const root = written(files);
	onTestFinished(() => {
		step('removing the tree');
		rmSync(root, {recursive: true, force: true});
		step('removed the tree');
	});
	step('wrote the files');
	return root;
}

/**
 * The tree of 3,000 files that the build tests share, written once for the file, since they read it and never change
 * it. A test that changes its tree writes its own.
 */
let large = '';

beforeAll(() => {
	large = written(3000);
});

afterAll(() => {
	rmSync(large, {recursive: true, force: true});
});

/** Opens a handle on `root`, which the test closes once it finishes. */
function opened(root: string): Kartta {
	const kartta = Kartta.open(root);
	step('opened');
	onTestFinished(() => {
		step('closing');
		kartta.close();
		step('closed');
	});
	return kartta;
}

function open(files = 1): Kartta {
	return opened(tree(files));
}

async function settled(promise: Promise<unknown>): Promise<unknown> {
	step('awaiting');
	try {
		return await promise;
	} catch (error) {
		if (error instanceof KarttaError) {
			return error;
		}
		throw error;
	}
}

async function code(promise: Promise<unknown>): Promise<string> {
	const outcome = await settled(promise);
	return outcome instanceof KarttaError ? outcome.code : 'ok';
}

async function walking(kartta: Kartta, signal?: AbortSignal): Promise<{built: Promise<unknown>}> {
	const built = kartta.build({}, signal === undefined ? {} : {signal});
	let state = await code(kartta.stats());
	while (state === 'not_built') {
		// Whether to poll again turns on the answer before, so the polls run one at a time.
		// oxlint-disable-next-line no-await-in-loop
		state = await code(kartta.stats());
	}
	if (state !== 'building') {
		throw new Error(`the build answered ${state} before a query saw it walking; give it a larger tree`);
	}
	return {built};
}

test('open refuses a root that is not a directory, with invalid_root', () => {
	const missing = path.join(tree(1), 'missing');

	expect(() => Kartta.open(missing)).toThrow(expect.objectContaining({name: 'KarttaError', code: 'invalid_root'}));
});

test('an error carries a code this package does not name', () => {
	expect(new KarttaError('a_code_of_a_newer_library', 'message')).toHaveProperty('code', 'a_code_of_a_newer_library');
});

test('a query before any build rejects with not_built', async () => {
	const kartta = open();

	expect(await code(kartta.stats())).toBe('not_built');
});

test('a limit of 0 answers the total alone, and one left out rejects with invalid_params', async () => {
	const kartta = open();
	await kartta.build();

	const counted = await kartta.definitions({name: 'f0', limit: 0});

	expect(counted.matches[0]).toMatchObject({found: [], total: 1});
	expect(await code(kartta.call('definitions', {name: 'f0'}))).toBe('invalid_params');
});

test('a line of 0 selects nothing and says why, and threads of 0 rejects', async () => {
	const kartta = open();

	expect(await code(kartta.build({threads: 0}))).toBe('invalid_params');

	await kartta.build();
	const answer = await kartta.callers({name: 'f0', line: 0, depth: 3});

	expect(answer.matches).toStrictEqual([]);
	expect(answer.unmatched?.kind).toBe('off_line');
});

test('a build keeps the graphs of a content index it cannot store and says why', async () => {
	const root = tree(1);
	const kartta = opened(root);

	const stats = await kartta.build({
		languages: ['go'],
		content_index: {storage: 'file', dir: path.join(root, 'gone')},
	});

	expect(stats.content_index).toBeUndefined();
	expect(stats.content_index_failure).toContain('gone');
	expect(stats.graphs).toHaveLength(1);
	expect(stats.errors.listed.map((failure) => failure.kind)).toStrictEqual(['content_index']);
});

test('a method the library does not answer rejects with unknown_method', async () => {
	const kartta = open();

	expect(await code(kartta.call('paths'))).toBe('unknown_method');
});

test('a thread the addon cannot start rejects with internal, the addon error its cause', () => {
	const script = `
		const {Kartta} = await import(process.argv[1]);
		const kartta = Kartta.open(process.argv[2]);
		const refused = async (call) => call.catch(({name, code, cause}) => [name, code, cause?.message]);
		console.log(JSON.stringify([await refused(kartta.build()), await refused(kartta.refresh())]));
		kartta.close();
	`;
	const module = new URL('../src/kartta.ts', import.meta.url).href;

	const stdout = execFileSync(
		process.execPath,
		['--no-warnings', '--input-type=module', '--eval', script, module, tree(1)],
		// constraint: Rust's std takes a spawned thread's stack size from here, and no OS maps a stack of 2^60 bytes
		{encoding: 'utf8', env: {...process.env, RUST_MIN_STACK: '1152921504606846976'}, timeout: 4000},
	);

	expect(stdout).toMatch(/\["KarttaError","internal","no thread for the build: [^"]+"\]/v);
	expect(stdout).toMatch(/\["KarttaError","internal","no thread for the refresh: [^"]+"\]/v);
});

test('an answer that cannot be read rejects with internal, the error its cause', async () => {
	const kartta = open();
	const tooLong = new RangeError('the answer is past the string limit');
	const decode = vi.spyOn(TextDecoder.prototype, 'decode').mockImplementationOnce(() => {
		throw tooLong;
	});
	onTestFinished(() => {
		decode.mockRestore();
	});

	await expect(kartta.stats()).rejects.toMatchObject({name: 'KarttaError', code: 'internal', cause: tooLong});
});

test('a signal aborted before the build rejects with its reason and keeps the index', async () => {
	const kartta = open();
	const built = await kartta.build();
	const reason = new Error('the caller gave up');

	await expect(kartta.build({}, {signal: AbortSignal.abort(reason)})).rejects.toBe(reason);

	expect(await kartta.stats()).toStrictEqual(built);
});

test('a signal aborted while the build walks cancels it, the reason its cause', async () => {
	const kartta = opened(large);
	const controller = new AbortController();
	const reason = new Error('the caller gave up');
	const {built} = await walking(kartta, controller.signal);

	controller.abort(reason);

	await expect(built).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled', cause: reason});
	expect(await code(kartta.stats())).toBeOneOf(['not_built', 'ok']);
	const rebuilt = await kartta.build();
	expect(rebuilt.entries).toBe(3000);
});

test('an abort right after the call cancels the build, entered or not, unless it finished', async () => {
	const kartta = opened(large);
	const controller = new AbortController();
	const reason = new Error('the caller gave up');
	const sameReason: unknown = expect.toSatisfy((cause) => cause === reason);

	const built = kartta.build({}, {signal: controller.signal});
	controller.abort(reason);

	expect(await settled(built)).toBeOneOf([
		expect.objectContaining({entries: 3000}),
		expect.objectContaining({code: 'build_cancelled', cause: sameReason}),
	]);
});

test('close cancels the build in flight, a closed error its cause', async () => {
	const kartta = opened(large);
	const {built} = await walking(kartta);

	kartta.close();

	const outcome = await settled(built);
	expect(outcome).toBeInstanceOf(KarttaError);
	expect(outcome).toHaveProperty('code', 'build_cancelled');
	expect(outcome).toHaveProperty('cause', expect.any(KarttaError));
	expect(outcome).toHaveProperty('cause.code', 'closed');
});

test('close right after a build call cancels it, entered or not, unless it finished', async () => {
	const kartta = opened(large);

	const built = kartta.build();
	kartta.close();

	expect(await settled(built)).toBeOneOf([
		expect.objectContaining({entries: 3000}),
		expect.objectContaining({code: 'build_cancelled'}),
	]);
});

test('close right after a build through call cancels it too, unless it finished', async () => {
	const kartta = opened(large);

	const built = kartta.call('build');
	kartta.close();

	expect(await settled(built)).toBeOneOf([
		expect.objectContaining({entries: 3000}),
		expect.objectContaining({code: 'build_cancelled'}),
	]);
});

test('an abort stops its build and not a build through call given its binding id as an id', async () => {
	const kartta = opened(large);
	const controller = new AbortController();

	const untyped = kartta.call('build', {id: 2});
	const typed = kartta.build({}, {signal: controller.signal});
	controller.abort();

	await expect(typed).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled'});
	expect(await untyped).toMatchObject({entries: 3000});
});

test("a cancel naming a raw build's id stops it and not a build of the handle's in flight", async () => {
	const kartta = opened(large);
	const {built} = await walking(kartta);
	const id = 1;

	const raw = settled(kartta.call('build', {id}));
	await kartta.cancel({build: id});

	expect(await settled(built)).toMatchObject({entries: 3000});
	expect(await raw).toMatchObject({name: 'KarttaError', code: 'build_cancelled'});
});

test("a cancel naming a finished raw build's id leaves the handle's next build running", async () => {
	const kartta = open();
	const id = 2;
	await kartta.call('build', {id});

	await kartta.cancel({build: id});

	expect(await kartta.build()).toMatchObject({entries: 1});
});

test("a cancel naming the id a build's options give stops that build", async () => {
	const kartta = open();
	const id = 78;

	await kartta.cancel({build: id});

	await expect(kartta.build({id})).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled'});
});

test('cancel with no id stops the build in flight', async () => {
	const kartta = opened(large);
	const {built} = await walking(kartta);

	const {stopped} = await kartta.cancel();

	expect(stopped).toBe(true);
	await expect(built).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled'});
});

/** A handle built with stamps over a tree of one file, the tree's root, and what the build indexed. */
async function stamped(): Promise<{kartta: Kartta; root: string; built: Stats}> {
	const root = tree(1);
	const kartta = opened(root);
	const built = await kartta.build({stamps: true});
	step('built');
	return {kartta, root, built};
}

/** Settles once `call` settles, with true whether it answered or rejected with a {@link KarttaError}. */
async function finished(call: Promise<unknown>): Promise<true> {
	await settled(call);
	return true;
}

/**
 * Sends cancels naming no call until one stops a call, which only a call that entered the library can be, and answers
 * true; or answers false once `call` settled first. A cancel answers on the calling thread, so each first waits a turn
 * of the event loop, in which `call` can settle.
 */
async function cancelOnceEntered(kartta: Kartta, call: Promise<unknown>): Promise<boolean> {
	const done = finished(call);
	let stopped = false;
	while (!stopped) {
		// Whether to cancel again turns on the answer before, so the cancels run one at a time.
		// oxlint-disable-next-line no-await-in-loop
		if (await Promise.race([done, setImmediate(false)])) {
			return false;
		}
		// oxlint-disable-next-line no-await-in-loop
		({stopped} = await kartta.cancel());
	}
	return true;
}

/** Writes `files` new files into `root`, which a refresh of the whole tree then reads. */
function grow(root: string, files: number): void {
	for (let file = 0; file < files; file += 1) {
		writeFileSync(path.join(root, `new${file}.go`), `package main\n\nfunc new${file}() {}\n`);
	}
}

test('a refresh of a handle built without stamps rejects with no_stamps', async () => {
	const kartta = open();
	await kartta.build();

	expect(await code(kartta.refresh())).toBe('no_stamps');
});

test('a refresh takes in the files added since the build', async () => {
	const {kartta, root} = await stamped();
	grow(root, 2);

	const refreshed = await kartta.refresh({paths: ['new0.go', 'new1.go']});

	expect(refreshed).toMatchObject({walked: 2, tree: false, added: 2});
	const stats = await kartta.stats();
	expect(stats.entries).toBe(3);
});

test('an abort right after a refresh call cancels it, entered or not, and leaves the index', async () => {
	const {kartta, root, built} = await stamped();
	grow(root, 3000);
	const controller = new AbortController();
	const reason = new Error('the caller gave up');

	const refreshed = kartta.refresh({}, {signal: controller.signal});
	controller.abort(reason);

	await expect(refreshed).rejects.toMatchObject({name: 'KarttaError', code: 'refresh_cancelled', cause: reason});
	expect(await kartta.stats()).toStrictEqual(built);
});

test('a signal aborted before the refresh rejects with its reason', async () => {
	const {kartta} = await stamped();
	const reason = new Error('the caller gave up');

	await expect(kartta.refresh({}, {signal: AbortSignal.abort(reason)})).rejects.toBe(reason);
});

test('close right after a refresh call cancels it, entered or not, a closed error its cause', async () => {
	const {kartta, root} = await stamped();
	grow(root, 3000);

	const refreshed = kartta.refresh();
	kartta.close();

	const outcome = await settled(refreshed);
	expect(outcome).toBeInstanceOf(KarttaError);
	expect(outcome).toHaveProperty('code', 'refresh_cancelled');
	expect(outcome).toHaveProperty('cause.code', 'closed');
});

test('close right after a refresh through call cancels it too', async () => {
	const {kartta, root} = await stamped();
	grow(root, 3000);

	const refreshed = kartta.call('refresh', {id: 7});
	kartta.close();

	expect(await settled(refreshed)).toMatchObject({code: 'refresh_cancelled'});
});

test('cancel with no id stops the refresh that entered, and leaves the index', async () => {
	const {kartta, root, built} = await stamped();
	grow(root, 3000);
	const refreshed = kartta.refresh();

	expect(await cancelOnceEntered(kartta, refreshed)).toBe(true);
	await expect(refreshed).rejects.toMatchObject({name: 'KarttaError', code: 'refresh_cancelled'});
	expect(await kartta.stats()).toStrictEqual(built);
});

/** Counts the threads of this process named kartta-watch, the name the library gives a watch's thread. */
function watchThreads(): number {
	const tasks = '/proc/self/task';
	return readdirSync(tasks).filter((task) => {
		try {
			return readFileSync(path.join(tasks, task, 'comm'), 'utf8').trim() === 'kartta-watch';
		} catch {
			// The thread ended between the listing and the read, so it watches nothing.
			return false;
		}
	}).length;
}

test('a watched build takes an edit in without a refresh', {timeout: 30_000}, async () => {
	const root = tree(1);
	const kartta = Kartta.open(root);
	onTestFinished(() => {
		kartta.close();
	});
	const built = await kartta.build({watch: true});

	grow(root, 1);

	// FSEvents can report the tree's own making, a moment before the build, after the watch began; that names the root,
	// which the watch takes in by a restart, so the restarts it counts vary.
	expect(built.watch).toMatchObject({following: true});
	await expect.poll(async () => kartta.stats(), {timeout: 20_000}).toMatchObject({entries: 2});
});

test.skipIf(process.platform !== 'linux')('close ends the watch thread', {timeout: 30_000}, async () => {
	const kartta = opened(tree(1));
	await kartta.build({watch: true});
	// constraint: build answers once the watch thread exists, and std names a thread from inside it, so the name can lag
	await expect.poll(watchThreads, {timeout: 20_000}).toBeGreaterThan(0);

	kartta.close();

	await expect.poll(watchThreads, {timeout: 20_000}).toBe(0);
});

const derive = promisify(pbkdf2);

/**
 * Holds every thread of libuv's pool for a while, each with a key derivation, so a query called meanwhile waits for one
 * in the pool's queue; settles once the threads are free.
 */
async function poolHeld(): Promise<void> {
	const threads = Number(process.env.UV_THREADPOOL_SIZE ?? '4');
	await Promise.all(Array.from({length: threads}, async () => derive('kartta', 'salt', 1_000_000, 64, 'sha512')));
}

test('close cancels a grep queued for a pool thread, which then answers at once', async () => {
	const kartta = open(200);
	await kartta.build();
	const held = poolHeld();

	const grepped = kartta.grep({patterns: ['f0\\(']});
	kartta.close();

	expect(await grepped).toMatchObject({cancelled: true, files: [], candidates: 200, searched: 0, unsearched: 200});
	await held;
});

test('close cancels a grep through call queued for a pool thread too', async () => {
	const kartta = open(200);
	await kartta.build();
	const held = poolHeld();

	const grepped = kartta.call('grep', {patterns: ['f0\\('], id: 7});
	kartta.close();

	expect(await grepped).toMatchObject({cancelled: true, files: [], candidates: 200, searched: 0, unsearched: 200});
	await held;
});

test('an abort stops a grep queued for a pool thread, and the next grep answers whole', async () => {
	const kartta = open(200);
	await kartta.build();
	const held = poolHeld();
	const controller = new AbortController();

	const grepped = kartta.grep({patterns: ['f0\\(']}, {signal: controller.signal});
	controller.abort();

	expect(await grepped).toMatchObject({cancelled: true, files: [], candidates: 200, searched: 0, unsearched: 200});
	await held;
	expect(await kartta.grep({patterns: ['f0\\(']})).toMatchObject({cancelled: false, searched: 200, unsearched: 0});
});

test('a signal aborted before the grep rejects with its reason', async () => {
	const kartta = open();
	await kartta.build();
	const reason = new Error('the caller gave up');

	await expect(kartta.grep({patterns: ['f0']}, {signal: AbortSignal.abort(reason)})).rejects.toBe(reason);
});

test('every call after close rejects with closed, and closing again does nothing', async () => {
	const kartta = open();
	await kartta.build();

	kartta.close();
	kartta.close();

	const calls = [
		kartta.build(),
		kartta.build({}, {signal: AbortSignal.abort()}),
		kartta.stats(),
		kartta.version(),
		kartta.callers({name: 'f0', depth: 3}),
		kartta.grep({patterns: ['f0']}),
		kartta.call('cancel'),
	];
	expect(await Promise.all(calls.map(async (call) => code(call)))).toStrictEqual(Array.from(calls, () => 'closed'));
});

test('queries run side by side and each answers as it does alone', async () => {
	const kartta = open(200);
	await kartta.build({content_index: {storage: 'memory'}});
	const queries = [
		async () => kartta.callers({name: 'f0', depth: 5}),
		async () => kartta.grep({patterns: ['f1\\d\\('], count_all: true}),
		async () => kartta.fuzzy({query: 'f19'}),
		async () => kartta.list({limit: 0}),
		async () => kartta.stats(),
	];
	const alone: unknown[] = [];
	for (const query of queries) {
		// Each query must answer alone, so the next starts once it has.
		// oxlint-disable-next-line no-await-in-loop
		alone.push(await query());
	}

	const together = await Promise.all(Array.from({length: 40}, async (_, at) => queries[at % queries.length]!()));

	expect(together).toStrictEqual(together.map((_, at) => alone[at % alone.length]));
});

test('builds asked side by side run one at a time and each finishes', async () => {
	const kartta = open(200);

	const built = await Promise.all([kartta.build(), kartta.build(), kartta.build()]);

	expect(built.map((stats) => stats.entries)).toStrictEqual([200, 200, 200]);
});
