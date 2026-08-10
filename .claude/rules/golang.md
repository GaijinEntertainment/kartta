---
paths:
  - "**/*.go"
---

# Go Conventions

Simplicity is the highest Go virtue. Resist abstraction until the cost of not abstracting is proven. When in doubt,
write boring code. If existing code contradicts a rule below, follow the code and flag the divergence.

## Naming

- Name length scales with scope distance: loop index `i`; short local `r`, `b`, `ctx`; parameter `name`, `opts`;
  package-level `defaultTimeout`; exported `ErrNotFound`, `DefaultClient`.
- Receivers: 1-2 letter type abbreviation (`c` for `Client`), consistent across all methods. Never `self`/`this`.
- Initialisms all-caps: `userID`, `httpClient`, `xmlHTTPRequest`, `URL`, `API`.
- Packages: short, lowercase, singular, named by what they provide. Never `util`, `common`, `misc`, `shared`,
  `helpers`, `types`, `models`.
- No stutter: `widget.New()` not `widget.NewWidget()`; method on `*Project` is `Name()` not `ProjectName()`; don't
  encode type in name (`users` not `numUsers`).
- Getters without `Get` prefix; setters with `Set`: `u.Name()` / `u.SetName(n)`.
- One-method interfaces: method name + `-er` (`Reader`, `Stringer`). Honor canonical names: `String`, not `ToString`.
- Constants MixedCaps, named by role not value: `MaxRetries = 12`, never `ALL_CAPS` or `Twelve`.
- Unexported globals plain lowercase; unexported sentinels `errXxx`, exported `ErrXxx`; error types `XxxError`.
- Go source files: kebab-case (`stream-conn.go`, `job-stat.go`).

## Interfaces & Types

- Consumer-side interfaces: define where used, not where implemented. Producers return concrete types. No interfaces
  "for mocking" — wait for a concrete need.
- Small interfaces: 1-3 methods. Accept interfaces, return structs. Never pointer-to-interface.
- Compile-time verification: `var _ http.Handler = (*Handler)(nil)`.
- Pointer receiver when: mutates, contains a mutex, large struct, or in doubt. Value receiver for small immutable
  values and map/func/chan. Never mix receiver kinds on one type. Never pass `*map`, `*chan`, `*string`, `*io.Reader`.
- Design for a useful zero value (`var buf bytes.Buffer`); write constructors only when non-zero defaults required.
- Embed only when the promoted method set IS the intended API; otherwise named field. Never embed in public API
  structs unless the promoted surface is deliberate; never embed `sync.Mutex` — named field `mu`.
- `any` over `interface{}`; use it only when truly accepting any type — otherwise generics or concrete types.
- Defined types for semantics: `type UserID string` adds safety; `type MyString string` adds nothing.
- Prefer `int`; fixed-width ints only when a protocol/format/perf constraint demands the width.

## Declarations

- `var` for zero values, `:=` for initialized; top-level `var` with type omitted when obvious.
- Nil slice preferred (`var s []string`); `[]string{}` only when JSON must encode `[]` not `null`. Check emptiness
  with `len(s) == 0`. Pre-allocate when size known: `make([]T, 0, n)`, `make(map[K]V, n)`.
- Struct literals: always field names; omit only fields tagged `exhaustruct:"optional"` (see exhaustruct below).
  `&T{}` over `new(T)`.
- Enums: start `iota + 1` unless zero value is meaningful.
- Named results only when they disambiguate (`left, right *Node`) or document obligations (`cancel func()`); never
  just to enable naked returns.

## exhaustruct (enabled repo-wide)

Every field of a struct literal must be explicitly initialized unless the field is tagged optional.

- **Types we own** — declare optionality at the type definition, not at call sites. For every field, decide whether it
  is **required** or **optional** at construction time, and annotate optional fields with an `exhaustruct:"optional"`
  struct tag, combined with other tags as needed: `` `json:"name,omitempty" exhaustruct:"optional"` ``. Untagged fields
  are required — the linter forces callers to fill them. Optionality is a design decision: zero-value-is-meaningful
  fields, fields populated by later wiring (DI, post-construction setters), and genuinely optional inputs are optional;
  fields a caller _must_ provide for the value to be valid are required. Never suppress at the call site for a type we
  own — annotate the type.
- **External types** we do not control (SDKs, upstream API structs, option types with many optional fields): if the
  linter commonly fires on them, add a regex entry to `settings.exhaustruct.exclude` in `.golangci.yaml`. Inline
  `//nolint:exhaustruct` on an external type is a review blocker — extend the exclude list so the whole repo benefits.
- **Tests** follow the same rule: optionality tags on the type. Never put a file-level `//nolint:exhaustruct` atop a
  test file — a test literal tripping the linter means the type is missing tags. The one acceptable inline suppression
  is a deliberate improper-use literal (a test constructing an invalid value on purpose), justified in the comment.
- **Inline `//nolint:exhaustruct`** is a last resort for one-off partial literals that justify neither a type redesign
  nor an exclude entry, and **must** carry a written justification (`//nolint:exhaustruct // <reason>`); bare
  suppressions are not acceptable. A suppression that keeps recurring means missing tags or a missing exclude entry —
  fix the root cause.

## Functions

- Synchronous by default — let callers add concurrency.
- Return errors, never `os.Exit`/`log.Fatal` outside `main()`.
- Early return on error; happy path flat at minimum indentation, never in `else`.
- Accept `io.Reader`, not filenames. `defer` close of transient resources (`resp.Body`, `rows`, files).
- Context: always first parameter; never stored in structs; never in option structs; `context.Background()` only in
  `main()`/test setup.
- Long-running processes: `Run(ctx)` blocks; `Start(ctx)` returns immediately and spawns the goroutine.
- Constructors with 3+ optional params: option struct when callers typically set several options or options are
  shared across functions; functional options when most callers need zero, options need validation, or third-party
  extensibility matters. Options take parameters (`FailFast(true)`), not presence (`EnableFailFast()`).

## Errors & logging — golib (mandatory)

Use [`dev.gaijin.team/go/golib/e`](https://pkg.go.dev/dev.gaijin.team/go/golib/e) instead of
`fmt.Errorf`/`errors.New` for creating and wrapping errors, and
[`dev.gaijin.team/go/golib/logger`](https://pkg.go.dev/dev.gaijin.team/go/golib/logger) for logging.

- Sentinels are objects: `var ErrNotFound = e.New("not found")`; wrap the cause via `ErrNotFound.Wrap(err)`. Methods
  return new instances — sentinels are safe to reuse.
- New error wrapping a cause: `e.NewFrom("create service", err)`; convert foreign errors: `e.From(err)`.
- Structured context goes into fields, not the message: `e.NewFrom("query failed", err, fields.F("change", num))` or
  chained `.WithField(k, v)`; snake_case keys. Fields are for machines, the reason string is for humans.
- Render format is `<reason> (fields...): <wrapped>`. Reason strings: stable, lowercase, no trailing punctuation, no
  "failed to" prefix — they compose newest-to-oldest (`"read config: open file: permission denied"`).
- Don't repeat what the wrapped error already says; if annotation adds nothing, return `err` as-is.
- Wrap when the caller provided the failing input or the cause is part of your API contract; hide the cause when it's
  an implementation detail you don't want to commit to.
- Match with `errors.Is`/`errors.As`, never `==` or direct type assertion. `errors.Join` aggregates independent
  errors; each joined error is still built with `e`.
- Handle once: log or return, never both. Logging side: `e.Log(err, logger.Error)` and degrade gracefully.
- Always check errors; no in-band failure values (`-1`, `""`) — return `(T, error)` or `(T, bool)`.
- Type assertions: comma-ok form always.
- Deferred `Close()` errors: propagate if no prior error; ignore explicitly with `_ =` and a comment when justified.
- Don't panic; reserve for violated invariants. Internal panic/recover only when panics cannot escape the package and
  a deferred `recover` translates them to errors.

## golib/must

[`dev.gaijin.team/go/golib/must`](https://pkg.go.dev/dev.gaijin.team/go/golib/must) only where failure is a
programming bug: package initialization, static data (URLs, regexes). Never on user input, network, or runtime paths.

## Concurrency

- Every goroutine needs a predictable exit condition and a way to be joined. No fire-and-forget; no goroutines in
  `init()` — spawn in constructors with lifecycle management.
- `context.Context` is the primary cancellation mechanism; select on `ctx.Done()`. `sync.WaitGroup` joins; prefer
  `errgroup.WithContext` over manual WaitGroup + error collection.
- Don't pass request contexts to background goroutines — use `context.WithoutCancel(r.Context())`.
- Mutexes protect shared state; channels coordinate independent actors and transfer ownership.
- Channels: unbuffered or size 1 — larger buffers require written justification. Direction in signatures
  (`<-chan`/`chan<-`). Close from sender only; sends on closed channels panic. A nil channel blocks forever — use to
  disable `select` cases.
- `select` picks a ready case at random, not in source order — drain work after a stop signal if order matters.
- `mu sync.Mutex` named field, never copied, `defer mu.Unlock()`. Simple flags/counters: `atomic.Bool`/`atomic.Int64`.
- Data races: `append` on a shared slice with spare capacity races — copy first; map/slice assignment aliases the
  backing storage — `maps.Clone` inside the critical section; `%v` on a type whose `String()` locks the same mutex
  deadlocks — format fields directly.

## Gotchas

- `:=` in `if`/`for` blocks shadows outer variables — `err` especially. Use `=` when targeting outer scope.
- `defer` evaluates arguments immediately — capture with a closure. `defer` in a loop runs at function exit — extract
  the loop body into a function.
- `append` on a slice with spare capacity mutates the shared backing array — cap with `s[:len(s):len(s)]` or copy.
- `len(s)` is bytes, not runes; iterate runes with `range`; count with `utf8.RuneCountInString`.
- String concatenation in a loop: `strings.Builder` with `Grow`; `+` is fine for a few fixed strings.
- Copy slices/maps at API boundaries to prevent external mutation; never copy structs containing a mutex.
- Typed nil in an interface is non-nil — return literal `nil` from interface-returning functions.
- When code does the opposite of the common pattern (e.g. `err == nil` check), boost the signal with a comment.

## Testing

Full conventions in [golang-testing.md](golang-testing.md) (loads for `*_test.go`). When creating a test file:
black-box preferred — `foo_test.go` with `package foo_test`; white-box `foo_internal_test.go` with `package foo` as a
last resort; benchmarks `foo_benchmark_test.go` / `foo_benchmark_internal_test.go`.

## Structure & Docs

- `internal/` aggressively for anything outside the public API; `cmd/<name>` per binary; start flat and add
  directories only on proven need.
- Imports in three groups (enforced by gci): stdlib, external, then this module. Alias only on conflict. Blank imports
  only in `main`/tests.
- In-file order: types/consts/vars, constructor, exported methods, unexported methods, utilities — callers before
  callees. Test file adjacent to source.
- Breaking changes staged in separate commits: add new → migrate callers → remove old. Never mix a breaking change
  with new functionality.
- Every exported symbol gets a godoc comment starting with its name, complete sentences; `doc.go` for package docs;
  runnable `func Example...` for complex APIs. Update the doc comment in the same edit that changes behavior.
- Libraries never force global state — instance APIs; a global convenience wrapper is a thin proxy over an instance
  and belongs in binaries only.
