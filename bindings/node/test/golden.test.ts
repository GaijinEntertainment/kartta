import {fileURLToPath} from 'node:url';

import {expect, onTestFinished, test} from 'vite-plus/test';

import {Kartta, KarttaError, type Methods} from '../src/kartta.ts';
import analyzeChange from '../../../crates/kartta-ffi/tests/golden/analyze_change.json' with {type: 'json'};
import analyzeSegment from '../../../crates/kartta-ffi/tests/golden/analyze_segment.json' with {type: 'json'};
import build from '../../../crates/kartta-ffi/tests/golden/build.json' with {type: 'json'};
import callPath from '../../../crates/kartta-ffi/tests/golden/call_path.json' with {type: 'json'};
import callees from '../../../crates/kartta-ffi/tests/golden/callees.json' with {type: 'json'};
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
import references from '../../../crates/kartta-ffi/tests/golden/references.json' with {type: 'json'};
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
	resolution,
	search,
	sources,
	stats,
	version,
};

type Goldens = typeof goldens;

/** The paths of the fields `Wire` holds and `Typed` does not name, at any depth; `never` when it names them all. */
type Unnamed<Wire, Typed, Path extends string> =
	Wire extends ReadonlyArray<infer Item>
		? Typed extends ReadonlyArray<infer TypedItem>
			? Unnamed<Item, TypedItem, `${Path}[]`>
			: Path
		: Wire extends object
			? {
					[Key in keyof Wire & string]: Key extends keyof Typed
						? Unnamed<Wire[Key], NonNullable<Typed[Key]>, `${Path}.${Key}`>
						: `${Path}.${Key}`;
				}[keyof Wire & string]
			: never;

type UnnamedFields = {
	[M in keyof Methods]:
		| Unnamed<Goldens[M]['params'], Methods[M]['params'], `${M}.params`>
		| Unnamed<Goldens[M]['answer']['ok'], Methods[M]['answer'], `${M}.answer`>;
}[keyof Methods];

const typesNameEveryFieldTheGoldensHold: [UnnamedFields] extends [never] ? true : `unnamed ${UnnamedFields}` = true;
const answersHoldTheirTypes: {[M in keyof Methods]: {answer: {ok: Methods[M]['answer']}}} = goldens;

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
	resolution: async (kartta, params) => kartta.resolution(params),
	search: async (kartta, params) => kartta.search(params),
	sources: async (kartta, params) => kartta.sources(params),
	stats: async (kartta) => kartta.stats(),
	version: async (kartta) => kartta.version(),
};

const fixture = fileURLToPath(new URL('../../../crates/kartta-ffi/tests/fixture', import.meta.url));

test("the goldens' fields are the ones the wire types name", () => {
	expect(typesNameEveryFieldTheGoldensHold).toBe(true);
	expect(answersHoldTheirTypes).toBe(goldens);
});

test('the typed calls cover every method the library answers', async () => {
	const kartta = Kartta.open(fixture);
	onTestFinished(() => kartta.close());

	const {methods} = await kartta.version();

	expect(Object.keys(replays).toSorted()).toStrictEqual(methods);
});

test('every method answers its golden request through its typed call as pinned', async () => {
	const kartta = Kartta.open(fixture);
	onTestFinished(() => kartta.close());

	for (const [method, golden] of Object.entries(goldens)) {
		// SAFETY: goldens and replays share their keys, as the mapped types above require.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		const replay = replays[method as keyof Methods] as (kartta: Kartta, params: unknown) => Promise<unknown>;
		const answered = await replay(kartta, golden.params).then(
			(ok) => ({ok}),
			(error: unknown) =>
				error instanceof KarttaError ? {err: {code: error.code, message: error.message}} : Promise.reject(error),
		);

		expect(answered, method).toStrictEqual(golden.answer);
	}
});
