import {mkdtempSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';

import {expect, onTestFinished, test} from 'vite-plus/test';

import {Kartta, KarttaError} from '../src/kartta.ts';

function tree(files: number): string {
	const root = mkdtempSync(join(tmpdir(), 'kartta-node-'));
	onTestFinished(() => rmSync(root, {recursive: true, force: true}));
	for (let file = 0; file < files; file += 1) {
		writeFileSync(join(root, `f${file}.go`), `package main\n\nfunc f${file}() {\n\tf${(file + 1) % files}()\n}\n`);
	}
	return root;
}

function open(files = 1): Kartta {
	const kartta = Kartta.open(tree(files));
	onTestFinished(() => kartta.close());
	return kartta;
}

async function code(promise: Promise<unknown>): Promise<string> {
	return promise.then(
		() => 'ok',
		(error: unknown) => (error instanceof KarttaError ? error.code : Promise.reject(error)),
	);
}

async function walking(kartta: Kartta, signal?: AbortSignal): Promise<{built: Promise<unknown>}> {
	const built = kartta.build({}, signal === undefined ? {} : {signal});
	let state = await code(kartta.stats());
	while (state === 'not_built') {
		state = await code(kartta.stats());
	}
	if (state !== 'building') {
		throw new Error(`the build answered ${state} before a query saw it walking; give it a larger tree`);
	}
	return {built};
}

async function cancelledUnlessFinished(built: Promise<unknown>, files: number): Promise<KarttaError | undefined> {
	const outcome = await built.then(
		(answer) => answer,
		(error: unknown) => (error instanceof KarttaError ? error : Promise.reject(error)),
	);
	if (outcome instanceof KarttaError) {
		expect(outcome.code).toBe('build_cancelled');
		return outcome;
	}
	expect(outcome).toMatchObject({entries: files});
	return undefined;
}

test('open refuses a root that is not a directory, with invalid_root', () => {
	const missing = join(tree(1), 'missing');

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
	expect((await kartta.build()).entries).toBe(3000);
});

test('an abort right after the call cancels the build, entered or not, unless it finished', async () => {
	const kartta = open(3000);
	const controller = new AbortController();
	const reason = new Error('the caller gave up');

	const built = kartta.build({}, {signal: controller.signal});
	controller.abort(reason);

	const cancelled = await cancelledUnlessFinished(built, 3000);
	if (cancelled !== undefined) {
		expect(cancelled.cause).toBe(reason);
	}
});

test('close cancels the build in flight, a closed error its cause', async () => {
	const kartta = open(3000);
	const {built} = await walking(kartta);

	kartta.close();

	const error = await built.catch((rejection: unknown) => rejection);
	expect(error).toBeInstanceOf(KarttaError);
	expect(error).toHaveProperty('code', 'build_cancelled');
	expect(error).toHaveProperty('cause', expect.any(KarttaError));
	expect(error).toHaveProperty('cause.code', 'closed');
});

test('close right after a build call cancels it, entered or not, unless it finished', async () => {
	const kartta = open(3000);

	const built = kartta.build();
	kartta.close();

	await cancelledUnlessFinished(built, 3000);
});

test('close right after a build through call cancels it too, unless it finished', async () => {
	const kartta = open(3000);

	const built = kartta.call('build');
	kartta.close();

	await cancelledUnlessFinished(built, 3000);
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
	expect(await Promise.all(calls.map(code))).toStrictEqual(Array(calls.length).fill('closed'));
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
