import javascript from '@ver0/oxlint-config/javascript.js';
import node from '@ver0/oxlint-config/node.js';
import typescript from '@ver0/oxlint-config/typescript.js';
import vitest from '@ver0/oxlint-config/vitest.js';
import {defineConfig} from 'vite-plus';

export default defineConfig({
	fmt: {
		printWidth: 120,
		useTabs: true,
		tabWidth: 2,
		semi: true,
		singleQuote: true,
		trailingComma: 'all',
		bracketSpacing: false,
		ignorePatterns: ['**/*.md'],
	},
	lint: {
		extends: [javascript, typescript, node, vitest],
		options: {
			typeAware: true,
			typeCheck: true,
		},
		rules: {
			// Kartta, the addon's handle, AbortSignal and Promise are mutable types, and the wire types mirror plain JSON.
			'typescript/prefer-readonly-parameter-types': 'off',
		},
	},
});
