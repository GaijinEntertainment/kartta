/**
 * Copies into the package in the working directory what `npm pack` ships from outside it: the licence and, given a
 * target, that target's addon, built beside package.json by build.ts, and the third-party notices.
 *
 * @module
 */

import {copyFileSync} from 'node:fs';

const target = process.argv[2];
const repository = new URL('../../../', import.meta.url);

copyFileSync(new URL('LICENSE', repository), 'LICENSE');
if (target !== undefined) {
	copyFileSync(new URL('notices/THIRD-PARTY-NOTICES.txt', repository), 'THIRD-PARTY-NOTICES.txt');
	copyFileSync(new URL(`../kartta.${target}.node`, import.meta.url), `kartta.${target}.node`);
}
