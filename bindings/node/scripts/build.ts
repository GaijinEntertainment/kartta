/**
 * Builds the addon of crates/kartta-node for this host and puts it where the package loads it from, as
 * `kartta.<target>.node` beside package.json. `--release` builds it optimized.
 *
 * @module
 */

import {execFileSync} from 'node:child_process';
import {copyFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';

import {target} from '../src/target.ts';

type CargoMessage = {
	reason: string;
	target?: {name: string};
	filenames?: string[];
};

const profile = process.argv.includes('--release') ? ['--release'] : [];
const messages = execFileSync(
	'cargo',
	['build', '--locked', '-p', 'kartta-node', '--message-format=json-render-diagnostics', ...profile],
	{encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 1 << 26},
);
const library = messages
	.split('\n')
	.filter((line) => line.startsWith('{'))
	// SAFETY: cargo writes one JSON object a line under --message-format=json, each with a reason.
	.map((line) => JSON.parse(line) as CargoMessage)
	.find((message) => message.reason === 'compiler-artifact' && message.target?.name === 'kartta_node')
	?.filenames?.find((file) => /\.(?:dylib|so|dll)$/v.test(file));
if (library === undefined) {
	throw new Error('cargo reported no shared library for kartta-node');
}
const addon = fileURLToPath(new URL(`../kartta.${target()}.node`, import.meta.url));
copyFileSync(library, addon);
process.stdout.write(`${addon}\n`);
