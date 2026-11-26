import {execFileSync, execSync} from 'node:child_process';
import {copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

import {expect, onTestFinished, test} from 'vite-plus/test';

import {TARGETS, target} from '../src/target.ts';

type Platform = {
	version: string;
	os?: string[];
	cpu?: string[];
	libc?: string[];
	optional?: boolean;
};

type Package = Platform & {
	name: string;
	license: string;
	main: string;
	files: string[];
	scripts: Record<string, string>;
	optionalDependencies: Record<string, string>;
	publishConfig: {access: string};
};

type Lock = {name: string; packages: Record<string, Package>};

function text(relative: string): string {
	return readFileSync(new URL(`../${relative}`, import.meta.url), 'utf8');
}

// SAFETY: each path is a manifest or the lock of this repository, and the assertions below fail on a field it lacks.
// oxlint-disable-next-line typescript/no-unsafe-type-assertion
const root = JSON.parse(text('package.json')) as Package;
// SAFETY: as above.
// oxlint-disable-next-line typescript/no-unsafe-type-assertion
const lock = JSON.parse(text('package-lock.json')) as Lock;
const stage = fileURLToPath(new URL('../scripts/stage.ts', import.meta.url));
const dependencies = Object.fromEntries(TARGETS.map((name) => [`@gaijin/kartta-${name}`, root.version]));

function runStage(args: string[]): {directory: string; run: () => string} {
	const directory = mkdtempSync(path.join(tmpdir(), 'kartta-stage-'));
	onTestFinished(() => {
		rmSync(directory, {recursive: true});
	});
	return {
		directory,
		run: () => execFileSync(process.execPath, [stage, ...args], {cwd: directory, encoding: 'utf8', stdio: 'pipe'}),
	};
}

function selectors(name: string): Pick<Platform, 'cpu' | 'libc' | 'os'> {
	const [platform, arch, abi] = name.split('-');
	const libc = {gnu: {libc: ['glibc']}, musl: {libc: ['musl']}}[abi ?? ''];
	return {cpu: [arch ?? ''], os: [platform ?? ''], ...libc};
}

test('a package exists for every target and for no other', () => {
	expect(readdirSync(new URL('../npm/', import.meta.url)).toSorted()).toStrictEqual([...TARGETS]);
});

test('the main package publishes as public under the @gaijin scope', () => {
	expect(root).toMatchObject({name: '@gaijin/kartta', publishConfig: {access: 'public'}});
	expect(lock).toMatchObject({name: root.name, packages: {'': {name: root.name}}});
});

test('the main package depends on every target at its own version', () => {
	expect(root.optionalDependencies).toStrictEqual(dependencies);
	expect(lock.packages['']?.optionalDependencies).toStrictEqual(dependencies);
});

test.for(TARGETS)('the %s package ships its addon and notices and installs only on its host', (name) => {
	const addon = `kartta.${name}.node`;
	// SAFETY: as above.
	// oxlint-disable-next-line typescript/no-unsafe-type-assertion
	const manifest = JSON.parse(text(`npm/${name}/package.json`)) as Package;
	expect(manifest).toMatchObject({
		name: `@gaijin/kartta-${name}`,
		version: root.version,
		publishConfig: {access: 'public'},
		main: addon,
		files: [addon, 'THIRD-PARTY-NOTICES.txt'],
		scripts: {prepack: `node ../../scripts/stage.ts ${name}`},
		...selectors(name),
	});
	expect(manifest.libc).toStrictEqual(selectors(name).libc);
});

test.for(TARGETS)('the lock holds the %s package', (name) => {
	expect(lock.packages[`node_modules/@gaijin/kartta-${name}`]).toStrictEqual({
		version: root.version,
		license: root.license,
		optional: true,
		...selectors(name),
	});
});

test.for(TARGETS)('scripts/version.sh lists the %s package', (name) => {
	expect(text('../../scripts/version.sh')).toContain(`'bindings/node/npm/${name}/package.json|`);
});

test('the stage copies the licence alone for the main package', () => {
	const {directory, run} = runStage([]);
	run();
	expect(readdirSync(directory)).toStrictEqual(['LICENSE']);
});

test('the stage copies the licence, the notices and the addon for the host', () => {
	const {directory, run} = runStage([target()]);
	run();
	expect(readdirSync(directory).toSorted()).toStrictEqual(
		['LICENSE', 'THIRD-PARTY-NOTICES.txt', `kartta.${target()}.node`].toSorted(),
	);
});

test('the stage fails on a target with no addon built', () => {
	const {directory, run} = runStage(['linux-riscv64-musl']);
	expect(run).toThrow(/kartta\.linux-riscv64-musl\.node/v);
	expect(existsSync(path.join(directory, 'kartta.linux-riscv64-musl.node'))).toBe(false);
});

test('the package loads the addon from the package of its host when no build sits beside it', {timeout: 60_000}, () => {
	const directory = mkdtempSync(path.join(tmpdir(), 'kartta-load-'));
	onTestFinished(() => {
		rmSync(directory, {recursive: true, force: true});
	});
	const checkout = path.join(directory, 'kartta');
	cpSync(fileURLToPath(new URL('../src/', import.meta.url)), path.join(checkout, 'src'), {recursive: true});
	copyFileSync(fileURLToPath(new URL('../package.json', import.meta.url)), path.join(checkout, 'package.json'));
	const installed = path.join(directory, 'node_modules', `@gaijin/kartta-${target()}`);
	mkdirSync(installed, {recursive: true});
	copyFileSync(
		fileURLToPath(new URL(`../npm/${target()}/package.json`, import.meta.url)),
		path.join(installed, 'package.json'),
	);
	const addon = `kartta.${target()}.node`;
	copyFileSync(fileURLToPath(new URL(`../${addon}`, import.meta.url)), path.join(installed, addon));
	const script = `
		const {Kartta} = await import(process.argv[1]);
		const kartta = Kartta.open(process.argv[2]);
		console.log((await kartta.version()).version);
		kartta.close();
	`;
	const module = pathToFileURL(path.join(checkout, 'src', 'kartta.ts')).href;

	const stdout = execFileSync(
		process.execPath,
		['--no-warnings', '--input-type=module', '--eval', script, module, directory],
		{
			encoding: 'utf8',
		},
	);

	expect(stdout.trim()).toBe(root.version);
});

test('npm pack of the host package holds its addon, the notices and the licence', {timeout: 60_000}, () => {
	const addon = `kartta.${target()}.node`;
	const staged = ['LICENSE', 'THIRD-PARTY-NOTICES.txt', addon];
	const directory = fileURLToPath(new URL(`../npm/${target()}/`, import.meta.url));
	onTestFinished(() => {
		for (const file of staged) {
			rmSync(path.join(directory, file), {force: true});
		}
	});

	// constraint: on Windows npm is npm.cmd, which runs only through a shell (Node's child_process, "Spawning .bat and
	// .cmd files on Windows")
	const answer = execSync('npm pack --dry-run --json', {cwd: directory, encoding: 'utf8', stdio: 'pipe'});

	// SAFETY: npm answers one object a package under --json, and the assertion below fails on a field it lacks.
	// oxlint-disable-next-line typescript/no-unsafe-type-assertion
	const [pack] = JSON.parse(answer) as [{files: Array<{path: string}>}];
	expect(pack.files.map((file) => file.path).toSorted()).toStrictEqual([...staged, 'package.json'].toSorted());
});
