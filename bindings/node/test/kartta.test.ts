import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test, type TestContext } from "node:test";

import { Kartta, KarttaError } from "../src/kartta.ts";

function tree(t: TestContext, files: number): string {
  const root = mkdtempSync(join(tmpdir(), "kartta-node-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (let file = 0; file < files; file += 1) {
    writeFileSync(join(root, `f${file}.go`), `package main\n\nfunc f${file}() {\n\tf${(file + 1) % files}()\n}\n`);
  }
  return root;
}

function open(t: TestContext, files = 1): Kartta {
  const kartta = Kartta.open(tree(t, files));
  t.after(() => kartta.close());
  return kartta;
}

async function code(promise: Promise<unknown>): Promise<string> {
  return promise.then(
    () => "ok",
    (error: unknown) => (error instanceof KarttaError ? error.code : Promise.reject(error)),
  );
}

async function walking(kartta: Kartta, signal?: AbortSignal): Promise<{ built: Promise<unknown> }> {
  const built = kartta.build({}, signal === undefined ? {} : { signal });
  let state = await code(kartta.stats());
  while (state === "not_built") {
    state = await code(kartta.stats());
  }
  if (state !== "building") {
    throw new Error(`the build answered ${state} before a query saw it walking; give it a larger tree`);
  }
  return { built };
}

test("open refuses a root that is not a directory, with invalid_root", (t: TestContext) => {
  const missing = join(tree(t, 1), "missing");

  t.assert.throws(() => Kartta.open(missing), { name: "KarttaError", code: "invalid_root" });
});

test("a query before any build rejects with not_built", async (t: TestContext) => {
  const kartta = open(t);

  t.assert.strictEqual(await code(kartta.stats()), "not_built");
});

test("a method the library does not answer rejects with unknown_method", async (t: TestContext) => {
  const kartta = open(t);

  t.assert.strictEqual(await code(kartta.call("paths")), "unknown_method");
});

test("a signal aborted before the build rejects with its reason and keeps the index", async (t: TestContext) => {
  const kartta = open(t);
  const built = await kartta.build();
  const reason = new Error("the caller gave up");

  await t.assert.rejects(kartta.build({}, { signal: AbortSignal.abort(reason) }), (error) => error === reason);

  t.assert.deepStrictEqual(await kartta.stats(), built);
});

test("a signal aborted while the build walks cancels it, the reason its cause", async (t: TestContext) => {
  const kartta = open(t, 3000);
  const controller = new AbortController();
  const reason = new Error("the caller gave up");
  const { built } = await walking(kartta, controller.signal);

  controller.abort(reason);

  await t.assert.rejects(built, { name: "KarttaError", code: "build_cancelled", cause: reason });
  t.assert.strictEqual(await code(kartta.stats()), "not_built");
  t.assert.strictEqual((await kartta.build()).entries, 3000);
});

test("a signal aborted right after the call cancels the build, entered or not", async (t: TestContext) => {
  const kartta = open(t);
  const controller = new AbortController();
  const reason = new Error("the caller gave up");

  const built = kartta.build({}, { signal: controller.signal });
  controller.abort(reason);

  await t.assert.rejects(built, { name: "KarttaError", code: "build_cancelled", cause: reason });
});

test("close cancels the build in flight, a closed error its cause", async (t: TestContext) => {
  const kartta = open(t, 3000);
  const { built } = await walking(kartta);

  kartta.close();

  await t.assert.rejects(built, (error) => {
    t.assert.ok(error instanceof KarttaError);
    t.assert.strictEqual(error.code, "build_cancelled");
    t.assert.ok(error.cause instanceof KarttaError);
    t.assert.strictEqual(error.cause.code, "closed");
    return true;
  });
});

test("close right after a build call cancels it, entered or not", async (t: TestContext) => {
  const kartta = open(t);

  const built = kartta.build();
  kartta.close();

  await t.assert.rejects(built, { name: "KarttaError", code: "build_cancelled" });
});

test("close right after a build through call cancels it too", async (t: TestContext) => {
  const kartta = open(t);

  const built = kartta.call("build");
  kartta.close();

  await t.assert.rejects(built, { name: "KarttaError", code: "build_cancelled" });
});

test("a build through call takes an id of the handle's, so an abort stops another", async (t: TestContext) => {
  const kartta = open(t, 3000);
  const controller = new AbortController();

  const untyped = kartta.call("build", { id: 2 });
  const typed = kartta.build({}, { signal: controller.signal });
  controller.abort();

  await t.assert.rejects(typed, { name: "KarttaError", code: "build_cancelled" });
  t.assert.partialDeepStrictEqual(await untyped, { entries: 3000 });
});

test("cancel with no id stops the build in flight", async (t: TestContext) => {
  const kartta = open(t, 3000);
  const { built } = await walking(kartta);

  const { stopped } = await kartta.cancel();

  t.assert.strictEqual(stopped, true);
  await t.assert.rejects(built, { name: "KarttaError", code: "build_cancelled" });
});

test("every call after close rejects with closed, and closing again does nothing", async (t: TestContext) => {
  const kartta = open(t);
  await kartta.build();

  kartta.close();
  kartta.close();

  const calls = [
    kartta.build(),
    kartta.build({}, { signal: AbortSignal.abort() }),
    kartta.stats(),
    kartta.version(),
    kartta.callers({ name: "f0" }),
    kartta.grep({ patterns: ["f0"] }),
    kartta.call("cancel"),
  ];
  t.assert.deepStrictEqual(await Promise.all(calls.map(code)), Array(calls.length).fill("closed"));
});

test("queries run side by side and each answers as it does alone", async (t: TestContext) => {
  const kartta = open(t, 200);
  await kartta.build({ content_index: { storage: "memory" } });
  const queries = [
    () => kartta.callers({ name: "f0", depth: 5 }),
    () => kartta.grep({ patterns: ["f1\\d\\("], count_all: true }),
    () => kartta.fuzzy({ query: "f19" }),
    () => kartta.list({ limit: 0 }),
    () => kartta.stats(),
  ];
  const alone: unknown[] = [];
  for (const query of queries) {
    alone.push(await query());
  }

  const together = await Promise.all(Array.from({ length: 40 }, (_, at) => queries[at % queries.length]!()));

  t.assert.deepStrictEqual(
    together,
    together.map((_, at) => alone[at % alone.length]),
  );
});

test("builds asked side by side run one at a time and each finishes", async (t: TestContext) => {
  const kartta = open(t, 200);

  const built = await Promise.all([kartta.build(), kartta.build(), kartta.build()]);

  t.assert.deepStrictEqual(
    built.map((stats) => stats.entries),
    [200, 200, 200],
  );
});
