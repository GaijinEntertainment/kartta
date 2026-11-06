/**
 * Builds the addon of crates/kartta-node for this host and puts it where the package loads it from, as
 * `kartta.<target>.node` beside package.json. Its arguments go to `cargo build`: `--release` builds it optimized, and
 * `--profile dist` builds what a release ships.
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

const host = target();
// constraint: a Rust target for musl links the C library statically and builds no cdylib until told not to
const env = host.endsWith('-musl')
	? {...process.env, RUSTFLAGS: `${process.env.RUSTFLAGS ?? ''} -C target-feature=-crt-static`}
	: process.env;
const messages = execFileSync(
	'cargo',
	['build', '--locked', '-p', 'kartta-node', '--message-format=json-render-diagnostics', ...process.argv.slice(2)],
	{encoding: 'utf8', env, stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 1 << 26},
);
const library = messages
	.split('\n')
	.filter((line) => line.startsWith('{'))
	// SAFETY: cargo writes one JSON object a line under --message-format=json, each with a reason.
	// oxlint-disable-next-line typescript/no-unsafe-type-assertion
	.map((line) => JSON.parse(line) as CargoMessage)
	.find((message) => message.reason === 'compiler-artifact' && message.target?.name === 'kartta_node')
	?.filenames?.find((file) => /\.(?:dylib|so|dll)$/v.test(file));
if (library === undefined) {
	throw new Error('cargo reported no shared library for kartta-node');
}
const addon = fileURLToPath(new URL(`../kartta.${host}.node`, import.meta.url));
copyFileSync(library, addon);
process.stdout.write(`${addon}\n`);
