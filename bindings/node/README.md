# kartta

Node.js binding of the kartta code graph engine: a handle on a tree, one build that walks it, and every file, content
and graph query the engine answers, each answer carrying the counts of what a cap or a bound left out.

```ts
import {Kartta} from 'kartta';

const kartta = Kartta.open('/path/to/repo');
try {
	await kartta.build({content_index: {storage: 'memory'}}, {signal: AbortSignal.timeout(60_000)});
	const {matches} = await kartta.callers({name: 'main', depth: 3});
	const {files, total_lines} = await kartta.grep({patterns: ['TODO'], count_all: true});
} finally {
	kartta.close();
}
```

- `build` and `refresh` each run on a thread of their own and a query on libuv's thread pool, so none holds the event
  loop. An `AbortSignal` stops a running build, grep or refresh; `close` stops every build, grep and refresh in
  flight, except that of those yet to enter the library it stops only the latest 1,024 of each.
- `build({watch: true})` follows the tree: the library refreshes the index from each change the operating system
  reports, on a thread of its own, until the next `build` or `close`; `stats().watch` says whether it still does.
- A failure rejects with a `KarttaError` whose `code` is the engine's: `not_built`, `building`, `invalid_glob` and the
  rest that `src/wire.ts` lists, `closed` for a call on a closed handle, or `internal` with the original error as its
  `cause` for a failure of the addon, such as a thread that cannot start.
- Each method of the wire has a typed call, its params and answer spelled as the wire spells them; `call(method,
  params)` reaches the wire untyped.

The addon is a native library per platform: macOS on arm64, Linux on x64 and arm64 each with glibc and with musl, and
Windows on x64. Each ships in a package of its own, `kartta-<target>`, an optional dependency of this package, which npm
installs only where it matches and this package loads; a checkout builds the host's with `vp run build`, which needs the
Rust toolchain `rust-toolchain.toml` pins. `npm pack` in a target's directory under `npm/` packs the addon a build left
beside `package.json`, with the licence and the third-party notices.

A checkout's toolchain is the `vite-plus` the lockfile pins, installed by `npm ci --ignore-scripts`: `vp fmt` formats,
`vp lint` lints, `vp run typecheck` type-checks and `vp test` runs the tests, which load the addon `vp run build` built.
Without a global `vp`, `npx vp` runs the same one.
