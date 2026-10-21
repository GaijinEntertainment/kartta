import {mkdtempSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {setImmediate} from 'node:timers/promises';

import {expect, onTestFinished, test} from 'vite-plus/test';

import {Kartta, KarttaError, type Stats} from '../src/kartta.ts';

function tree(files: number): string {
	const root = mkdtempSync(path.join(tmpdir(), 'kartta-node-'));
	onTestFinished(() => {
		rmSync(root, {recursive: true, force: true});
	});
	for (let file = 0; file < files; file += 1) {
		writeFileSync(path.join(root, `f${file}.go`), `package main\n\nfunc f${file}() {\n\tf${(file + 1) % files}()\n}\n`);
	}
	return root;
}

function open(files = 1): Kartta {
	const kartta = Kartta.open(tree(files));
	onTestFinished(() => {
		kartta.close();
	});
	return kartta;
}

async function settled(promise: Promise<unknown>): Promise<unknown> {
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

test('a query before any build rejects with not_built', async () => {
	const kartta = open();

	expect(await code(kartta.stats())).toBe('not_built');
});

test('a method the library does not answer rejects with unknown_method', async () => {
	const kartta = open();

	expect(await code(kartta.call('paths'))).toBe('unknown_method');
});

test('a signal aborted before the build rejects with its reason and keeps the index', async () => {
	const kartta = open();
	const built = await kartta.build();
	const reason = new Error('the caller gave up');

	await expect(kartta.build({}, {signal: AbortSignal.abort(reason)})).rejects.toBe(reason);

	expect(await kartta.stats()).toStrictEqual(built);
});

test('a signal aborted while the build walks cancels it, the reason its cause', async () => {
	const kartta = open(3000);
	const controller = new AbortController();
	const reason = new Error('the caller gave up');
	const {built} = await walking(kartta, controller.signal);

	controller.abort(reason);

	await expect(built).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled', cause: reason});
	expect(await code(kartta.stats())).toBe('not_built');
	const rebuilt = await kartta.build();
	expect(rebuilt.entries).toBe(3000);
});

test('an abort right after the call cancels the build, entered or not, unless it finished', async () => {
	const kartta = open(3000);
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
	const kartta = open(3000);
	const {built} = await walking(kartta);

	kartta.close();

	const outcome = await settled(built);
	expect(outcome).toBeInstanceOf(KarttaError);
	expect(outcome).toHaveProperty('code', 'build_cancelled');
	expect(outcome).toHaveProperty('cause', expect.any(KarttaError));
	expect(outcome).toHaveProperty('cause.code', 'closed');
});

test('close right after a build call cancels it, entered or not, unless it finished', async () => {
	const kartta = open(3000);

	const built = kartta.build();
	kartta.close();

	expect(await settled(built)).toBeOneOf([
		expect.objectContaining({entries: 3000}),
		expect.objectContaining({code: 'build_cancelled'}),
	]);
});

test('close right after a build through call cancels it too, unless it finished', async () => {
	const kartta = open(3000);

	const built = kartta.call('build');
	kartta.close();

	expect(await settled(built)).toBeOneOf([
		expect.objectContaining({entries: 3000}),
		expect.objectContaining({code: 'build_cancelled'}),
	]);
});

test("a build through call takes an id of the handle's, so an abort stops another", async () => {
	const kartta = open(3000);
	const controller = new AbortController();

	const untyped = kartta.call('build', {id: 2});
	const typed = kartta.build({}, {signal: controller.signal});
	controller.abort();

	await expect(typed).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled'});
	expect(await untyped).toMatchObject({entries: 3000});
});

test('cancel with no id stops the build in flight', async () => {
	const kartta = open(3000);
	const {built} = await walking(kartta);

	const {stopped} = await kartta.cancel();

	expect(stopped).toBe(true);
	await expect(built).rejects.toMatchObject({name: 'KarttaError', code: 'build_cancelled'});
});

/** A handle built with stamps over a tree of one file, the tree's root, and what the build indexed. */
async function stamped(): Promise<{kartta: Kartta; root: string; built: Stats}> {
	const root = tree(1);
	const kartta = Kartta.open(root);
	onTestFinished(() => {
		kartta.close();
	});
	const built = await kartta.build({stamps: true});
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
		kartta.callers({name: 'f0'}),
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
