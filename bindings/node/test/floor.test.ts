import {readdirSync, readFileSync} from 'node:fs';

import {expect, test} from 'vite-plus/test';

type Manifest = {
	engines: {node: string};
	devDependencies: Record<string, string>;
};

function text(relative: string): string {
	return readFileSync(new URL(`../${relative}`, import.meta.url), 'utf8');
}

function manifest(relative: string): Manifest {
	// SAFETY: each path is a package.json of this repository, and the assertions below fail on a field it lacks.
	// oxlint-disable-next-line typescript/no-unsafe-type-assertion
	return JSON.parse(text(relative)) as Manifest;
}

const root = manifest('package.json');
const floor = /^>=(?<line>\d+)$/u.exec(root.engines.node)?.groups?.line;

test.for(readdirSync(new URL('../npm/', import.meta.url)))('the %s package asks for the floor', (target) => {
	expect(manifest(`npm/${target}/package.json`).engines.node).toBe(root.engines.node);
});

test('CI runs the line the floor names', () => {
	expect(text('.nvmrc').split('.')[0]).toBe(floor);
});

test('the Node types come from the line the floor names', () => {
	expect(root.devDependencies['@types/node']?.split('.')[0]).toBe(floor);
});
