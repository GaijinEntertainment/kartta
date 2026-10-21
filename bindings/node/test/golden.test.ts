import {fileURLToPath} from 'node:url';

import {expect, onTestFinished, test} from 'vite-plus/test';

import {Kartta, KarttaError, type Methods} from '../src/kartta.ts';
import analyzeChange from '../../../crates/kartta-ffi/tests/golden/analyze_change.json' with {type: 'json'};
import analyzeSegment from '../../../crates/kartta-ffi/tests/golden/analyze_segment.json' with {type: 'json'};
import build from '../../../crates/kartta-ffi/tests/golden/build.json' with {type: 'json'};
import callPathUnmatched from '../../../crates/kartta-ffi/tests/golden/call_path-unmatched.json' with {type: 'json'};
import callPath from '../../../crates/kartta-ffi/tests/golden/call_path.json' with {type: 'json'};
import callees from '../../../crates/kartta-ffi/tests/golden/callees.json' with {type: 'json'};
import callersOneEdit from '../../../crates/kartta-ffi/tests/golden/callers-one_edit.json' with {type: 'json'};
import callers from '../../../crates/kartta-ffi/tests/golden/callers.json' with {type: 'json'};
import cancel from '../../../crates/kartta-ffi/tests/golden/cancel.json' with {type: 'json'};
import deadCode from '../../../crates/kartta-ffi/tests/golden/dead_code.json' with {type: 'json'};
import definitions from '../../../crates/kartta-ffi/tests/golden/definitions.json' with {type: 'json'};
import dependents from '../../../crates/kartta-ffi/tests/golden/dependents.json' with {type: 'json'};
import fuzzy from '../../../crates/kartta-ffi/tests/golden/fuzzy.json' with {type: 'json'};
import glob from '../../../crates/kartta-ffi/tests/golden/glob.json' with {type: 'json'};
import grep from '../../../crates/kartta-ffi/tests/golden/grep.json' with {type: 'json'};
import implementations from '../../../crates/kartta-ffi/tests/golden/implementations.json' with {type: 'json'};
import list from '../../../crates/kartta-ffi/tests/golden/list.json' with {type: 'json'};
import outline from '../../../crates/kartta-ffi/tests/golden/outline.json' with {type: 'json'};
import referencesUnmatched from '../../../crates/kartta-ffi/tests/golden/references-unmatched.json' with {type: 'json'};
import references from '../../../crates/kartta-ffi/tests/golden/references.json' with {type: 'json'};
import refreshPaths from '../../../crates/kartta-ffi/tests/golden/refresh-paths.json' with {type: 'json'};
import refresh from '../../../crates/kartta-ffi/tests/golden/refresh.json' with {type: 'json'};
import resolution from '../../../crates/kartta-ffi/tests/golden/resolution.json' with {type: 'json'};
import search from '../../../crates/kartta-ffi/tests/golden/search.json' with {type: 'json'};
import sources from '../../../crates/kartta-ffi/tests/golden/sources.json' with {type: 'json'};
import stats from '../../../crates/kartta-ffi/tests/golden/stats.json' with {type: 'json'};
import version from '../../../crates/kartta-ffi/tests/golden/version.json' with {type: 'json'};

const goldens = {
	build,
	analyze_change: analyzeChange,
	analyze_segment: analyzeSegment,
	call_path: callPath,
	callees,
	callers,
	cancel,
	dead_code: deadCode,
	definitions,
	dependents,
	fuzzy,
	glob,
	grep,
	implementations,
	list,
	outline,
	references,
	refresh,
	resolution,
	search,
	sources,
	stats,
	version,
};

type Goldens = typeof goldens;

/** The goldens of the second request a method has one for, which reaches keys its first golden does not, by method. */
const secondGoldens = {
	call_path: callPathUnmatched,
	callers: callersOneEdit,
	references: referencesUnmatched,
	refresh: refreshPaths,
};

type Golden = {params: unknown; answer: {ok: unknown}};

/** The paths of the fields `Wire` holds and `Typed` does not name, at any depth; `never` when it names them all. */
type Unnamed<Wire, Typed, Path extends string> =
	Wire extends ReadonlyArray<infer Item>
		? Typed extends ReadonlyArray<infer TypedItem>
			? Unnamed<Item, TypedItem, `${Path}[]`>
			: Path
		: Wire extends Record<string, unknown>
			? {
					[Key in keyof Wire & string]: Key extends keyof Typed
						? Unnamed<Wire[Key], NonNullable<Typed[Key]>, `${Path}.${Key}`>
						: `${Path}.${Key}`;
				}[keyof Wire & string]
			: never;

/** The paths of the fields the goldens `G` hold, by method, that the method's types do not name. */
type UnnamedIn<G extends Partial<Record<keyof Methods, Golden>>> = {
	[M in keyof Methods & keyof G]:
		| Unnamed<NonNullable<G[M]>['params'], Methods[M]['params'], `${M}.params`>
		| Unnamed<NonNullable<G[M]>['answer']['ok'], Methods[M]['answer'], `${M}.answer`>;
}[keyof Methods & keyof G];

/** `true` when `Paths`, the paths of unnamed fields, holds none, else the paths. */
type NamesEvery<Paths extends string> = [Paths] extends [never] ? true : `unnamed ${Paths}`;

const typesNameEveryFieldTheGoldensHold: NamesEvery<UnnamedIn<Goldens>> = true;
const typesNameEveryFieldTheSecondGoldensHold: NamesEvery<UnnamedIn<typeof secondGoldens>> = true;
const answersHoldTheirTypes: {[M in keyof Methods]: {answer: {ok: Methods[M]['answer']}}} = goldens;
const secondAnswersHoldTheirTypes: {[M in keyof typeof secondGoldens]: {answer: {ok: Methods[M]['answer']}}} =
	secondGoldens;

type Replay<M extends keyof Methods> = (kartta: Kartta, params: Methods[M]['params']) => Promise<Methods[M]['answer']>;

const replays: {[M in keyof Methods]: Replay<M>} = {
	build: async (kartta, params) => kartta.build(params),
	analyze_change: async (kartta, params) => kartta.analyzeChange(params),
	analyze_segment: async (kartta, params) => kartta.analyzeSegment(params),
	call_path: async (kartta, params) => kartta.callPath(params),
	callees: async (kartta, params) => kartta.callees(params),
	callers: async (kartta, params) => kartta.callers(params),
	cancel: async (kartta, params) => kartta.cancel(params),
	dead_code: async (kartta, params) => kartta.deadCode(params),
	definitions: async (kartta, params) => kartta.definitions(params),
	dependents: async (kartta, params) => kartta.dependents(params),
	fuzzy: async (kartta, params) => kartta.fuzzy(params),
	glob: async (kartta, params) => kartta.glob(params),
	grep: async (kartta, params) => kartta.grep(params),
	implementations: async (kartta, params) => kartta.implementations(params),
	list: async (kartta, params) => kartta.list(params),
	outline: async (kartta, params) => kartta.outline(params),
	references: async (kartta, params) => kartta.references(params),
	refresh: async (kartta, params) => kartta.refresh(params),
	resolution: async (kartta, params) => kartta.resolution(params),
	search: async (kartta, params) => kartta.search(params),
	sources: async (kartta, params) => kartta.sources(params),
	stats: async (kartta) => kartta.stats(),
	version: async (kartta) => kartta.version(),
};

const fixture = fileURLToPath(new URL('../../../crates/kartta-ffi/tests/fixture', import.meta.url));

// constraint: search's scores pass through ln_1p, which Apple's libm and glibc round apart in the last bits
const scoreTolerance = 1e-12;

async function envelope(answer: Promise<unknown>): Promise<unknown> {
	try {
		return {ok: await answer};
	} catch (error) {
		if (error instanceof KarttaError) {
			return {err: {code: error.code, message: error.message}};
		}
		throw error;
	}
}

function isJsonObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null;
}

function scoresAgree(answer: unknown, pinned: unknown): boolean {
	return (
		typeof answer === 'number' &&
		typeof pinned === 'number' &&
		Math.abs(answer - pinned) <= scoreTolerance * Math.max(Math.abs(answer), Math.abs(pinned))
	);
}

function pinAgreeingScores(answer: unknown, golden: unknown): unknown {
	if (Array.isArray(answer)) {
		return answer.map((item: unknown, index) =>
			pinAgreeingScores(item, Array.isArray(golden) ? golden[index] : undefined),
		);
	}
	if (!isJsonObject(answer)) {
		return answer;
	}
	const pinned = isJsonObject(golden) ? golden : {};
	return Object.fromEntries(
		Object.entries(answer).map(([key, value]) => [
			key,
			key === 'score' && scoresAgree(value, pinned[key]) ? pinned[key] : pinAgreeingScores(value, pinned[key]),
		]),
	);
}

test("the goldens' fields are the ones the wire types name", () => {
	expect(typesNameEveryFieldTheGoldensHold).toBe(true);
	expect(typesNameEveryFieldTheSecondGoldensHold).toBe(true);
	expect(answersHoldTheirTypes).toBe(goldens);
	expect(secondAnswersHoldTheirTypes).toBe(secondGoldens);
});

test('the typed calls cover every method the library answers', async () => {
	const kartta = Kartta.open(fixture);
	onTestFinished(() => {
		kartta.close();
	});

	const {methods} = await kartta.version();

	expect(Object.keys(replays).toSorted()).toStrictEqual(methods);
});

test('every method answers its golden request through its typed call as pinned', async () => {
	const kartta = Kartta.open(fixture);
	onTestFinished(() => {
		kartta.close();
	});

	const answered: Record<string, unknown> = {};
	for (const [method, golden] of Object.entries(goldens)) {
		// SAFETY: goldens and replays share their keys, as the mapped types above require.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		const replay = replays[method as keyof Methods] as (kartta: Kartta, params: unknown) => Promise<unknown>;
		// Each replay asks what the build golden built, so the replays run in the goldens' order, one at a time.
		// oxlint-disable-next-line no-await-in-loop
		answered[method] = await envelope(replay(kartta, golden.params));
	}
	const answeredSecond: Record<string, unknown> = {};
	for (const [method, golden] of Object.entries(secondGoldens)) {
		// SAFETY: secondGoldens is keyed by methods, as the mapped type above requires.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		const replay = replays[method as keyof Methods] as (kartta: Kartta, params: unknown) => Promise<unknown>;
		// The second requests ask what the build golden built, after it, one at a time.
		// oxlint-disable-next-line no-await-in-loop
		answeredSecond[method] = await envelope(replay(kartta, golden.params));
	}
	const pinned = Object.fromEntries(Object.entries(goldens).map(([method, golden]) => [method, golden.answer]));
	const pinnedSecond = Object.fromEntries(
		Object.entries(secondGoldens).map(([method, golden]) => [method, golden.answer]),
	);

	expect(pinAgreeingScores(answered, pinned)).toStrictEqual(pinned);
	expect(pinAgreeingScores(answeredSecond, pinnedSecond)).toStrictEqual(pinnedSecond);
});

const pinnedScore = 3.5799073758139874;
const pinnedWeight = 0.7024774156511924;

function hits(score: number, weight: number): unknown {
	return {hits: [{score, weight}]};
}

test('a score that differs in its last digit agrees with the pinned one', () => {
	const golden = hits(pinnedScore, pinnedWeight);

	expect(pinAgreeingScores(hits(3.579907375813988, pinnedWeight), golden)).toStrictEqual(golden);
});

test('a score that differs in a real digit does not agree', () => {
	const golden = hits(pinnedScore, pinnedWeight);

	expect(pinAgreeingScores(hits(3.579907375913988, pinnedWeight), golden)).not.toStrictEqual(golden);
});

test('a number outside a score must match exactly', () => {
	const golden = hits(pinnedScore, pinnedWeight);

	expect(pinAgreeingScores(hits(pinnedScore, 0.7024774156511925), golden)).not.toStrictEqual(golden);
});
