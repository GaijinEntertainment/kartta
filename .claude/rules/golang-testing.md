---
paths:
  - "**/*_test.go"
---

# Go Testing

## Files & Packages

- Black-box preferred: `foo_test.go` with `package foo_test` — validates the public API. White-box
  `foo_internal_test.go` with `package foo` is a last resort for logic unreachable through the API.
- Benchmarks: `foo_benchmark_test.go` / `foo_benchmark_internal_test.go`. Test file adjacent to source.
- Test-double packages: `<pkg>test` (e.g. `creditcardtest`) with `Stub`/`Fake`/`Spy` types; prefix with the doubled
  type name when several types need doubles.

## Assertions — testify only

- `require` for error checks and nil guards (stops the test); `assert` for independent value checks (reports and
  continues); `suite` when a group needs shared lifecycle.
- `require.Equal`/`assert.Equal` for structs and slices — never raw `reflect.DeepEqual`. `assert.ElementsMatch` for
  order-independent slices.
- Error asserts are semantic: `require.ErrorIs`/`ErrorAs`; `require.ErrorContains` when no sentinel exists; never
  exact error-string equality.
- Don't assert on serialization output (`json.Marshal` ordering is not stable) — parse and compare.

## Table Tests

- Slice `tests`, case `tt`, inputs `give*`, outputs `want*`; always `t.Run` with descriptive names; field names in
  struct literals; omit zero-value fields unless they add context.
- Uniform logic only — every row uses every field. Split into separate `Test` functions when cases need different
  setup/mocks, conditional assertions, or fields used by only some rows.
- Bare `{}` blocks for sequential grouping when separate reporting isn't needed; `t.Run` when you need parallelism,
  selective runs, or per-scenario reporting.
- Test complementary operations together (Put + Get) when it cuts duplication; split when failure modes are
  independent.

## Failure Semantics

- Prefer `t.Error` to report all failures at once; `t.Fatal` only for setup failures or when continuing is
  meaningless. In subtests `t.Fatal` stops only the subtest.
- `t.Fatal`/`t.FailNow` never in spawned goroutines — `t.Errorf` + `return`. Parallel subtests (`t.Parallel()`) are
  not spawned goroutines; `t.Fatal` is safe there.
- Helpers call `t.Helper()` so failures point at the caller — but not in assert-like wrappers, where it hides the
  failing line.

## Isolation & Determinism

- Never write to the source dir or mutate process env — `t.TempDir()`, `t.Setenv()` (the test cache keys off file
  mtime and env values).
- Setup scoped to the tests that need it: no `init()` or package-level test data; `sync.Once` for expensive shared
  setup.
- No `time.Sleep` for synchronization — channels, WaitGroups, or a poll loop with deadline
  (`select` on done vs `time.After`).
- Concurrent code runs under `-race`; exclude a file with `//go:build !race` only with written justification.

## Integrations

- Prefer live services (DB, cache, broker) over synthetic mocks; gate slow/external tests behind env vars and
  `t.Skip` when unset.
- `httptest.NewRequest`/`NewRecorder` for handlers; `httptest.NewServer` for clients against a fake server;
  `testing/iotest` (`ErrReader`, `OneByteReader`) for reader resilience.

## Benchmarks & Examples

- `b.Loop()` for the loop; `b.ResetTimer()` after expensive setup; `b.ReportAllocs()`; sink results to a
  package-level var so the compiler can't eliminate the call; `benchstat`/`-benchtime=5s` for stable micro-benchmarks.
- Runnable `func Example...` with `// Output:` for complex APIs — godoc renders them, `go test` verifies them.
