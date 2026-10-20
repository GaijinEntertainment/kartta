# kartta

Node.js binding of the kartta code graph engine: a handle on a tree, one build that walks it, and every file, content
and graph query the engine answers, each answer carrying the counts of what a cap or a bound left out.

```ts
import {Kartta} from 'kartta';

const kartta = Kartta.open('/path/to/repo');
try {
	await kartta.build({content_index: {storage: 'memory'}}, {signal: AbortSignal.timeout(60_000)});
	const {matches} = await kartta.callers({name: 'main'});
	const {files, total_lines} = await kartta.grep({patterns: ['TODO'], count_all: true});
} finally {
	kartta.close();
}
```

- `build` runs on a thread of its own and a query on libuv's thread pool, so neither holds the event loop. An
  `AbortSignal` stops a running build; `close` stops every build in flight.
- A failure rejects with a `KarttaError` whose `code` is the engine's: `not_built`, `building`, `invalid_glob` and the
  rest that `src/wire.ts` lists, or `closed` for a call on a closed handle.
- Each method of the wire has a typed call, its params and answer spelled as the wire spells them; `call(method,
  params)` reaches the wire untyped.

The addon is a native library per platform: macOS on arm64, and Linux with glibc on x64 and arm64. Each ships in a
package of its own, `kartta-<target>`, which this package loads; a checkout builds the host's with `vp run build`,
which needs the Rust toolchain `rust-toolchain.toml` pins.

A checkout's toolchain is the `vite-plus` the lockfile pins, installed by `npm ci --ignore-scripts`: `vp fmt` formats,
`vp lint` lints, `vp run typecheck` type-checks and `vp test` runs the tests, which load the addon `vp run build` built.
Without a global `vp`, `npx vp` runs the same one.
