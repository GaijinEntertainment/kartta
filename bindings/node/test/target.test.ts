import {expect, test, vi} from 'vite-plus/test';

import {TARGETS, target} from '../src/target.ts';

const glibc = {header: {glibcVersionRuntime: '2.41'}};
const musl = {header: {}};

test.for([
	['darwin', 'arm64', musl, 'darwin-arm64'],
	['linux', 'arm64', glibc, 'linux-arm64-gnu'],
	['linux', 'arm64', musl, 'linux-arm64-musl'],
	['linux', 'x64', glibc, 'linux-x64-gnu'],
	['linux', 'x64', musl, 'linux-x64-musl'],
	['win32', 'x64', musl, 'win32-x64-msvc'],
] as const)('%s on %s with %o is %s', ([platform, arch, report, expected]) => {
	expect(target({platform, arch, report: {getReport: () => report}})).toBe(expected);
});

test('a target outside the support matrix keeps its platform and arch', () => {
	expect(target({platform: 'freebsd', arch: 'x64', report: {getReport: () => musl}})).toBe('freebsd-x64');
});

test('the report skips the network and leaves the setting as it was', () => {
	const seen: Array<boolean | undefined> = [];
	const report = {
		excludeNetwork: false,
		getReport: (): unknown => {
			seen.push(report.excludeNetwork);
			return glibc;
		},
	};
	target({platform: 'linux', arch: 'x64', report});
	expect(seen).toStrictEqual([true]);
	expect(report.excludeNetwork).toBe(false);
});

test('a host with no addon fails naming itself and every target that ships', async () => {
	vi.doMock(import('../src/target.ts'), async (original) => ({
		...(await original()),
		target: () => 'linux-riscv64-musl',
	}));
	const {Kartta} = await import('../src/kartta.ts');
	const opening = (): unknown => Kartta.open('.');
	expect(opening).toThrow(new RegExp(`no addon for linux-riscv64-musl; it ships one for ${TARGETS.join(', ')},`, 'v'));
	vi.doUnmock(import('../src/target.ts'));
});
