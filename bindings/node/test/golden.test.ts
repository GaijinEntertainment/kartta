import {fileURLToPath} from 'node:url';

import {expect, onTestFinished, test} from 'vite-plus/test';

import {Kartta, KarttaError, type Methods} from '../src/kartta.ts';
import analyzeChangeConsumerDirs from '../../../crates/kartta-ffi/tests/golden/analyze_change-consumer_dirs.json' with {type: 'json'};
import analyzeChangeHotspots from '../../../crates/kartta-ffi/tests/golden/analyze_change-hotspots.json' with {type: 'json'};
import analyzeChangeUsers from '../../../crates/kartta-ffi/tests/golden/analyze_change-users.json' with {type: 'json'};
import analyzeChange from '../../../crates/kartta-ffi/tests/golden/analyze_change.json' with {type: 'json'};
import analyzeSegmentBounds from '../../../crates/kartta-ffi/tests/golden/analyze_segment-bounds.json' with {type: 'json'};
import analyzeSegmentUsers from '../../../crates/kartta-ffi/tests/golden/analyze_segment-users.json' with {type: 'json'};
import analyzeSegment from '../../../crates/kartta-ffi/tests/golden/analyze_segment.json' with {type: 'json'};
import buildEveryOption from '../../../crates/kartta-ffi/tests/golden/build-every_option.json' with {type: 'json'};
import build from '../../../crates/kartta-ffi/tests/golden/build.json' with {type: 'json'};
import callPathCandidates from '../../../crates/kartta-ffi/tests/golden/call_path-candidates.json' with {type: 'json'};
import callPathFromUnmatched from '../../../crates/kartta-ffi/tests/golden/call_path-from_unmatched.json' with {type: 'json'};
import callPathUnmatched from '../../../crates/kartta-ffi/tests/golden/call_path-unmatched.json' with {type: 'json'};
import callPath from '../../../crates/kartta-ffi/tests/golden/call_path.json' with {type: 'json'};
import calleesOffLine from '../../../crates/kartta-ffi/tests/golden/callees-off_line.json' with {type: 'json'};
import callees from '../../../crates/kartta-ffi/tests/golden/callees.json' with {type: 'json'};
import callersCandidates from '../../../crates/kartta-ffi/tests/golden/callers-candidates.json' with {type: 'json'};
import callersFileScope from '../../../crates/kartta-ffi/tests/golden/callers-file_scope.json' with {type: 'json'};
import callersInvalidParams from '../../../crates/kartta-ffi/tests/golden/callers-invalid_params.json' with {type: 'json'};
import callersOneEdit from '../../../crates/kartta-ffi/tests/golden/callers-one_edit.json' with {type: 'json'};
import callersSelfCall from '../../../crates/kartta-ffi/tests/golden/callers-self_call.json' with {type: 'json'};
import callers from '../../../crates/kartta-ffi/tests/golden/callers.json' with {type: 'json'};
import cancelBindingId from '../../../crates/kartta-ffi/tests/golden/cancel-binding_id.json' with {type: 'json'};
import cancelGrepRefresh from '../../../crates/kartta-ffi/tests/golden/cancel-grep_refresh.json' with {type: 'json'};
import cancel from '../../../crates/kartta-ffi/tests/golden/cancel.json' with {type: 'json'};
import deadCodePathPrefix from '../../../crates/kartta-ffi/tests/golden/dead_code-path_prefix.json' with {type: 'json'};
import deadCode from '../../../crates/kartta-ffi/tests/golden/dead_code.json' with {type: 'json'};
import definitionsLimit from '../../../crates/kartta-ffi/tests/golden/definitions-limit.json' with {type: 'json'};
import definitionsOneEdit from '../../../crates/kartta-ffi/tests/golden/definitions-one_edit.json' with {type: 'json'};
import definitions from '../../../crates/kartta-ffi/tests/golden/definitions.json' with {type: 'json'};
import dependentsBounds from '../../../crates/kartta-ffi/tests/golden/dependents-bounds.json' with {type: 'json'};
import dependents from '../../../crates/kartta-ffi/tests/golden/dependents.json' with {type: 'json'};
import fuzzyGlobs from '../../../crates/kartta-ffi/tests/golden/fuzzy-globs.json' with {type: 'json'};
import fuzzy from '../../../crates/kartta-ffi/tests/golden/fuzzy.json' with {type: 'json'};
import globInvalidGlob from '../../../crates/kartta-ffi/tests/golden/glob-invalid_glob.json' with {type: 'json'};
import glob from '../../../crates/kartta-ffi/tests/golden/glob.json' with {type: 'json'};
import grepCut from '../../../crates/kartta-ffi/tests/golden/grep-cut.json' with {type: 'json'};
import grepInvalidPattern from '../../../crates/kartta-ffi/tests/golden/grep-invalid_pattern.json' with {type: 'json'};
import grep from '../../../crates/kartta-ffi/tests/golden/grep.json' with {type: 'json'};
import implementationsCandidates from '../../../crates/kartta-ffi/tests/golden/implementations-candidates.json' with {type: 'json'};
import implementationsOtherKind from '../../../crates/kartta-ffi/tests/golden/implementations-other_kind.json' with {type: 'json'};
import implementations from '../../../crates/kartta-ffi/tests/golden/implementations.json' with {type: 'json'};
import listCollapse from '../../../crates/kartta-ffi/tests/golden/list-collapse.json' with {type: 'json'};
import listNotADirectory from '../../../crates/kartta-ffi/tests/golden/list-not_a_directory.json' with {type: 'json'};
import listNotFound from '../../../crates/kartta-ffi/tests/golden/list-not_found.json' with {type: 'json'};
import list from '../../../crates/kartta-ffi/tests/golden/list.json' with {type: 'json'};
import outline from '../../../crates/kartta-ffi/tests/golden/outline.json' with {type: 'json'};
import referencesAccess from '../../../crates/kartta-ffi/tests/golden/references-access.json' with {type: 'json'};
import referencesType from '../../../crates/kartta-ffi/tests/golden/references-type.json' with {type: 'json'};
import referencesUnmatched from '../../../crates/kartta-ffi/tests/golden/references-unmatched.json' with {type: 'json'};
import referencesUnrecordedData from '../../../crates/kartta-ffi/tests/golden/references-unrecorded_data.json' with {type: 'json'};
import references from '../../../crates/kartta-ffi/tests/golden/references.json' with {type: 'json'};
import refreshIds from '../../../crates/kartta-ffi/tests/golden/refresh-ids.json' with {type: 'json'};
import refreshPaths from '../../../crates/kartta-ffi/tests/golden/refresh-paths.json' with {type: 'json'};
import refresh from '../../../crates/kartta-ffi/tests/golden/refresh.json' with {type: 'json'};
import resolutionPastBound from '../../../crates/kartta-ffi/tests/golden/resolution-past_bound.json' with {type: 'json'};
import resolution from '../../../crates/kartta-ffi/tests/golden/resolution.json' with {type: 'json'};
import searchFuzzy from '../../../crates/kartta-ffi/tests/golden/search-fuzzy.json' with {type: 'json'};
import search from '../../../crates/kartta-ffi/tests/golden/search.json' with {type: 'json'};
import sourcesCutLines from '../../../crates/kartta-ffi/tests/golden/sources-cut_lines.json' with {type: 'json'};
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

/** The goldens of each further request that answers, by file name: the method, then what the request pins. */
const laterGoldens = {
	'analyze_change-consumer_dirs': analyzeChangeConsumerDirs,
	'analyze_change-hotspots': analyzeChangeHotspots,
	'analyze_change-users': analyzeChangeUsers,
	'analyze_segment-bounds': analyzeSegmentBounds,
	'analyze_segment-users': analyzeSegmentUsers,
	'build-every_option': buildEveryOption,
	'call_path-candidates': callPathCandidates,
	'call_path-from_unmatched': callPathFromUnmatched,
	'call_path-unmatched': callPathUnmatched,
	'callees-off_line': calleesOffLine,
	'callers-candidates': callersCandidates,
	'callers-file_scope': callersFileScope,
	'callers-one_edit': callersOneEdit,
	'callers-self_call': callersSelfCall,
	'cancel-binding_id': cancelBindingId,
	'cancel-grep_refresh': cancelGrepRefresh,
	'dead_code-path_prefix': deadCodePathPrefix,
	'definitions-limit': definitionsLimit,
	'definitions-one_edit': definitionsOneEdit,
	'dependents-bounds': dependentsBounds,
	'fuzzy-globs': fuzzyGlobs,
	'grep-cut': grepCut,
	'implementations-candidates': implementationsCandidates,
	'implementations-other_kind': implementationsOtherKind,
	'list-collapse': listCollapse,
	'references-access': referencesAccess,
	'references-type': referencesType,
	'references-unmatched': referencesUnmatched,
	'references-unrecorded_data': referencesUnrecordedData,
	'refresh-ids': refreshIds,
	'refresh-paths': refreshPaths,
	'resolution-past_bound': resolutionPastBound,
	'search-fuzzy': searchFuzzy,
	'sources-cut_lines': sourcesCutLines,
};

/** The goldens of the requests a method refuses, by file name, as `laterGoldens` holds the rest. */
const failedGoldens = {
	'callers-invalid_params': callersInvalidParams,
	'glob-invalid_glob': globInvalidGlob,
	'grep-invalid_pattern': grepInvalidPattern,
	'list-not_a_directory': listNotADirectory,
	'list-not_found': listNotFound,
};

type Golden = {params: unknown; answer: {ok: unknown}};

/** The method a golden's file name opens with. */
type MethodOf<Stem extends string> = Stem extends `${infer Method extends keyof Methods}-${string}` ? Method : never;

/** The members of the union `Typed` that name `Key`. */
type Naming<Typed, Key extends string> = Typed extends unknown ? (Key extends keyof Typed ? Typed : never) : never;

/** The paths of the fields `Wire` holds and `Typed` does not name, at any depth; `never` when it names them all. */
type Unnamed<Wire, Typed, Path extends string> =
	Wire extends ReadonlyArray<infer Item>
		? Typed extends ReadonlyArray<infer TypedItem>
			? Unnamed<Item, TypedItem, `${Path}[]`>
			: Path
		: Wire extends Record<string, unknown>
			? {
					[Key in keyof Wire & string]: [Naming<Typed, Key>] extends [never]
						? `${Path}.${Key}`
						: Unnamed<Wire[Key], NonNullable<Naming<Typed, Key>[Key & keyof Naming<Typed, Key>]>, `${Path}.${Key}`>;
				}[keyof Wire & string]
			: never;

/** The paths of the fields the goldens `G` hold, by method, that the method's types do not name. */
type UnnamedIn<G extends Partial<Record<keyof Methods, Golden>>> = {
	[M in keyof Methods & keyof G]:
		| Unnamed<NonNullable<G[M]>['params'], Methods[M]['params'], `${M}.params`>
		| Unnamed<NonNullable<G[M]>['answer']['ok'], Methods[M]['answer'], `${M}.answer`>;
}[keyof Methods & keyof G];

/** The paths of the fields the goldens `G` hold, by file name, that their method's types do not name. */
type UnnamedInLater<G extends Record<string, Golden>> = {
	[Stem in keyof G & string]:
		| Unnamed<G[Stem]['params'], Methods[MethodOf<Stem>]['params'], `${Stem}.params`>
		| Unnamed<G[Stem]['answer']['ok'], Methods[MethodOf<Stem>]['answer'], `${Stem}.answer`>;
}[keyof G & string];

/** The paths of the fields the params of the goldens `G` hold, which answer no type, that the params do not name. */
type UnnamedInFailed<G extends Record<string, {params: unknown}>> = {
	[Stem in keyof G & string]: Unnamed<G[Stem]['params'], Methods[MethodOf<Stem>]['params'], `${Stem}.params`>;
}[keyof G & string];

/** `true` when `Paths`, the paths of unnamed fields, holds none, else the paths. */
type NamesEvery<Paths extends string> = [Paths] extends [never] ? true : `unnamed ${Paths}`;

const typesNameEveryFieldTheGoldensHold: NamesEvery<UnnamedIn<Goldens>> = true;
const typesNameEveryFieldTheLaterGoldensHold: NamesEvery<UnnamedInLater<typeof laterGoldens>> = true;
const typesNameEveryFieldTheFailedGoldensHold: NamesEvery<UnnamedInFailed<typeof failedGoldens>> = true;
const answersHoldTheirTypes: {[M in keyof Methods]: {answer: {ok: Methods[M]['answer']}}} = goldens;
const laterAnswersHoldTheirTypes: {
	[Stem in keyof typeof laterGoldens]: {answer: {ok: Methods[MethodOf<Stem>]['answer']}};
} = laterGoldens;

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
	expect(typesNameEveryFieldTheLaterGoldensHold).toBe(true);
	expect(typesNameEveryFieldTheFailedGoldensHold).toBe(true);
	expect(answersHoldTheirTypes).toBe(goldens);
	expect(laterAnswersHoldTheirTypes).toBe(laterGoldens);
});

test('the typed calls cover every method the library answers', async () => {
	const kartta = Kartta.open(fixture);
	onTestFinished(() => {
		kartta.close();
	});

	const {methods} = await kartta.version();

	expect(Object.keys(replays).toSorted()).toStrictEqual(methods);
});

test('every golden request answers through its typed call as pinned', async () => {
	const kartta = Kartta.open(fixture);
	onTestFinished(() => {
		kartta.close();
	});

	const everyGolden = Object.entries({...goldens, ...laterGoldens, ...failedGoldens});
	const answered: Record<string, unknown> = {};
	for (const [stem, golden] of everyGolden) {
		// SAFETY: a golden's file name opens with its method, and replays holds every method, as the types above require.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		const method = stem.split('-', 1)[0] as keyof Methods;
		// SAFETY: as above.
		// oxlint-disable-next-line typescript/no-unsafe-type-assertion
		const replay = replays[method] as (kartta: Kartta, params: unknown) => Promise<unknown>;
		// Each replay asks what the build golden built, so the replays run in the goldens' order, one at a time.
		// oxlint-disable-next-line no-await-in-loop
		answered[stem] = await envelope(replay(kartta, golden.params));
	}
	const pinned = Object.fromEntries(everyGolden.map(([stem, golden]) => [stem, golden.answer]));

	expect(pinAgreeingScores(answered, pinned)).toStrictEqual(pinned);
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
