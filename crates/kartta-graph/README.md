# kartta-graph

A language-agnostic code graph: the definitions, calls and uses of one language family's files.

A `Graph` takes in one `kartta_lang::Extraction` per file, as the languages built on `kartta-lang` produce them, and
answers what a file defines, where a name is defined, and who calls what. It names no language: what differs between
languages reaches it as the extractions and the `Conventions` it is built with.

## A file at a time

`Graph::insert` replaces whatever the graph held for a path and `Graph::remove` forgets one. Files live in slots a
removal frees for the next insertion, and a definition is addressed by its file's slot and its place in the file, so a
graph follows a tree that changes without being built again, and one file going in or out renumbers nothing else.
Names are interned once per graph.

## Calls are resolved when a query asks

A file keeps its call sites and uses as its language wrote them, and a query resolves the sites of the name it asks
about. A call binds at the first tier of a cascade that binds it, in this order: the local the caller declares, the
import binding a receiver names, or that brings a bare call's name in from a module's file, the type a receiver's path
reaches, a callee written qualified, the caller's base class or the owner a receiver names, then the caller's own file,
its imports, its package, its includes and the re-export chains through them, and last the caller's own owner, the one
callable of the name and the callables nearest the caller; [ADR 3.7](../../design-docs/adr/3.7-call-resolution.md),
[ADR 3.171](../../design-docs/adr/3.171-receiver-call-binding.md) and
[ADR 3.172](../../design-docs/adr/3.172-bare-call-binding.md) state the tiers.
Where a file's extraction lists the packages
its package is built with, the tiers that bind by name take a definition of another package only from one of them and
outside its test code; a call through an owner whose members of the name all lie outside them binds nothing, and a name
they turned a definition of away binds no one callable and no guess outside the caller's directories. Where includes are
transitive, the one callable of the name binds only a call whose file, or a file that includes it directly, reaches that
callable or a declaration of it. A call from outside test code binds no test definition as the one callable of the name
or the one nearest the caller, and a test definition still counts there, so where test code shares a name, most often to
stub a builtin or a library outside the graph, no other definition of it is the one callable; where the definitions
outside test code are one type, such a call binds that type, which a test's type of its name stands in for. An import of
code outside the repository binds nothing to a call through it, nor, where a package is not a directory, to a bare call
of the name it imports that the caller's file does not answer, as `useState()` with `useState` from `react`. An import
binds a package's directory, whose exports a call through it reaches, or a module's file: a call through such an import,
or of the name it imports, binds what the file exports at its top level under the name, a type only where the file
defines nothing else of the name there; a call from the file or from a file it includes, directly or through others,
binds a definition the file keeps private too, as a Rust module and the modules below it see; a test definition binds
only a call from test code that shares an include root with the file, as a Rust crate's test build alone compiles it for
that crate's tests, and from any of its modules where the file exports it; a macro the file does not export answers no
call through it. A name the file does not define there binds the one exported macro of the name that the files it
includes define, directly or through others, as Rust's `#[macro_export]` places a macro at its crate's root whatever
module defines it; two such macros, or none, leave the call to the tiers below. A call does not say whether it invokes a
macro, so where the module also answers the name otherwise, through an exported definition of the file or of the files
it includes or through a `use` of the name that the file binds, this tier binds neither and leaves the call to the tiers
below. A dot import of a module's file, a Rust glob, binds a bare call the same way below the caller's own file, which
shadows it, where one such import answers the name. Where no import binds a receiver and the receiver names the module
that a file the caller's file includes directly declares, the call binds that file's definition of the name ahead of the
caller's own file, as daslang's `checks::f()` beside `require checks` does, and a use qualified so binds it too. A
member call, `x.f()`, binds a free function only where its language's conventions let one stand for what a value holds,
or for what a module its receiver names holds, as in Rust's `walk::run()`, and `f` names no method the language's
runtime gives its own values, as `trim` in `line.trim()` does; such a runtime method binds a member of its name only
where scope reaches one, never the nearest, and the one callable of the name only where its receiver spells that
callable's owner, as `session.seek()` binds `Session.seek`. A method call of a method the language's standard library
declares, on a value whose type the graph cannot name, binds the one callable of its name only as a guess unless its
receiver spells that callable's owner, as `found.start()` binds `Watcher::start`. Where a language writes a bare
receiver only for a class, as PHP's `Mockery::mock()` does, a call on a bare name that no type of the graph is named
binds through the file's imports, includes and re-exports alone, never the caller's own file or owner, the one callable
or the nearest. A member call binds no free variable or constant, whose name no property of a value reads. Where a
language says its calls never name a namespace, no call binds one; the namespace still counts among the definitions of
its name wherever a definition the call's shape rules out does, so a function named like it is not the one callable of
the name. Where a language lists the receivers it writes for a value of the caller's own type, as `self` and `other`, a
member call binds a member of the caller's own owner only on one of them. Where a language keeps a class's own methods
apart from its instances' methods of one name, as Ruby's `Formula.build` and `Formula#build`, a member call or use on
the class binds the class's own method of an owner first, and one on any other receiver the instances' method first.
Where a language's includes are transitive, as C++'s are, a call of a name at most 32 definitions share that no included
file defines binds the one file the includes reach in turn that defines it, a member call only a member. Where scopes
nest, as C++'s do, a member call on an operand of a declared class binds the member of the class of that name nearest
the caller's file, and of namesakes equally near, the one the class's spelling names from the caller's scope outward: of
a template and its specializations, which share one qualified name, the explicit specialization that takes the template
arguments the spelling writes, else the partial specialization that takes them as written, else the primary template
where no partial specialization may take them, else the first, as of copies of one class, and namesakes of other names
it names none of leave the call ambiguous; where that class declares no member of the name, the call binds the member of
its bases that the lookup of a bare call in its member finds. There, a call in a member on a field, `lock.acquire()` or
`lock->acquire()`, binds the same way through the class the field is declared as, where the caller's class, else the
nearest of its bases that declares the name, declares that field
([ADR 3.47](../../design-docs/adr/3.47-cpp-field-calls.md)).
Where a language says so, a member call on a value takes nothing from the callables nearest the caller. A use outside a
call binds more narrowly: through the import its qualifier names, else a package-level definition of its own file, of a
dot import, or of its package directory, and where a language's uses reach its imports, of the one included file that
declares it or of a re-export from one. A member a use takes on a value binds a field or a method through the type path
its language spells, or of the owner a path names in its own file or in one included file, or the definition outside
every owner of the one included module whose name the path ends, where a qualified name starts with its module. A path
none of those reach binds nothing but in three cases: an empty path, a Go path the graph cannot follow to a type, and a
C++ path to a type the graph holds that declares no such member, nor does a base. Those, where the language allows it,
bind only the one field of the name their own file declares outside other bodies; a guess by package, import or family
binds a namesake too often to be taken. A member of `this` or `self` binds the member of the class around the use, else
one of a base that every base searched before it was found for. Where includes are transitive and a bare name in a
member reads the class first, as in C++, the tiers follow the includes instead: a member of the own receiver, and a bare
name in a member, binds the own class's field, else the field of the one base the class-scope lookup of a bare call
certainly finds; a member on a value binds the field of the class its declaration names or of the one base of it that
declares the name, else, in those three cases, the one field of the name in the file, then in its includes, then, for a
name at most 32 fields share, in the files those reach. A member defined in a header its class's header includes, an
inline implementation header, reaches the classes its declaration's file sees too. Of namesake classes that hold the
field, the own class is the method's, the one a member defined outside it declares it in, else the one its scope's
template arguments name, and a declared one is the one its spelling names from the use's scope outward, of a template
and its specializations the one its template arguments name as a call's do; other namesakes of one qualified name bind
none, nor do namesakes the spelling names none of; a method taken as a value, `&Box<T>::fill`, binds as a call does, the
specialization its template arguments name, else the first of namesakes of one qualified name. Of namesakes without the
field, the bases of the one the spelling names are searched, else those of the first. A bare name past the class binds
the one namespace value of the name its file, else its includes, else, for a name at most 32 values share, the files
those reach declare. Each tier's confidence is a constant of `kartta_graph::confidence`. No edge is stored, so nothing a
file holds goes stale when another file changes.

## Answers

- `Graph::outline` — what a file defines, ordered by line, an enclosing definition before those it holds; `None` for a
  file the graph does not hold, which an empty outline is not.
- `Graph::definitions` — where a bare or qualified name is defined, nearest to the asking file first, cut to a limit and
  counting what the limit left out.
- `Graph::search` — the definitions whose names and file paths share the most words with a query, ranked by BM25
  weighed by the share of the query's words each holds and doubled for those in query order, a name that is the query
  first, generated and vendored code demoted, counting every match, those a name matched rather than a path, the
  query's words no definition holds and those past the eighth. When the words match fewer definitions than the limit,
  a fill-up adds those whose name or qualified name spells the query in order, and counts them. Each file keeps the
  distinct words of its definitions and path, gathered by the first search, so building spends nothing on search and a
  search splits only the names of the files that may hold a word it asks for.
- `Graph::callers` and `Graph::callees` — the callers or callees of a callable, to a depth, above a confidence floor and
  under a symbol cap, counting what the cap, the depth bound and the floor left out. Each also gives the callable's
  call of itself, which the symbols reached do not list. The callers also count the calls of the callable's name that
  bound none because several of its definitions may be meant.
- `Graph::references` — the sites that call or use a symbol, a field included, by file and line, each marked a read, a
  write, an update or a use as a type; it can keep one access, where the readers take the updates and the uses as a type
  too, the writers take the updates too, the updates and the uses as a type stand alone, and none keeps a call. It
  counts every site by kind whatever it keeps, and counts what the floor and the limit left out, the uses of a member's
  name it could not tell apart, and the calls of the symbol's name that `Graph::callers` counts as left ambiguous
  between several of its definitions.
- `Graph::call_path` — the shortest chain of calls from one callable to another, counting what the cap, the depth bound
  and the floor left out.
- `Graph::dependents` — the files that include a file, transitively, nearest first, to a depth and under a file cap,
  counting the includers the cap turned away and the files reached whose own includers the walk did not read; `None`
  for a file the graph does not hold. The includers of each file are kept as an index, updated as files go in and out.
- `Graph::implementations` — the types that name a type as a parent, transitively, to a depth and cut to a limit,
  counting the implementors the limit left out and the types reached whose own implementors the query did not look
  for. A written parent names the type its language's scopes reach, a type alias among them: a type of its package
  directory or of the directory its import binds where a package is a directory, the type its owner chain or its
  enclosing scopes reach first where a language has them, else the type of that name nearest the file.
- `Graph::analyze_segment` — the symbols a range of a file's lines overlaps, each with its direct callers and callees,
  at most the query's call limit of each listed and all of them counted, and the blast of a change to it: the callers a
  walk over its callers reaches by depth, the files they and the file's includers sit in, and a reach band, counting
  what the cap, the depth bound and the floor left out, and the calls of its name left ambiguous as `Graph::callers`
  counts them; `None` for a file the graph does not hold. A field, a variable or a constant also has its users, the
  symbols that read, write or call it, listed and counted as its callers are, and its walk starts from them, as a
  function's starts from its callers. Each name's sites are resolved once for the whole segment.
- `Graph::analyze_change` — what a change to a set of line regions reaches: the symbols they overlap, a declaration and
  its definition counting once, the callers a walk over each one's callers reaches, the footprint those walks cover
  together, and the hotspots two or more of them reach, each list cut at the query's limit and all of them counted. Each
  changed symbol lists its direct callers while the query's consumer limit or fewer pass the floor, and counts them by
  directory past that, a field, a variable or a constant its direct users in their place, the symbols that read, write
  or call it, from which its walk starts, and counts the calls of its name left ambiguous as `Graph::callers` does. The
  regions whose file the graph does not hold, those that overlap no symbol, and the walks the cap stopped are counted.
  `Change::merge` ranks the answers of several graphs together, as a change across language families needs.
- `Graph::dead_code` — the definitions no call and no use reaches, in path order, cut to a limit and counting every one
  found, those found reached, and those it did not judge, by reason: of a kind not asked for; test, generated, entry
  and vendored code; a kind no call and no use shows used; a definition a call or a use that binds nothing, or binds
  only by proximity, may reach; a method a call through another type may reach; and a definition no use reaches that
  a library or a host reads by its name, as a Go field whose struct tag hands it to a library, a field of a struct a
  marshal call takes, or a Go plugin's export. A field, a variable and a constant are judged through their reads and
  writes where the language records one wherever code reads or writes one, as Go's conventions say, and a call's
  receiver leaves no value unjudged, since such a language records the read of a value a call goes through. Every call
  and use that spells a judged name is resolved, so a query over a whole graph costs what resolving its calls and uses
  costs.
- `Graph::stats` — what the graph holds: its files and those with no definition, its definitions by kind, the calls,
  local calls and uses inside definitions, and the includes, counted when asked.
- `Graph::resolution` — how every call site binds: the sites bound at each confidence, the caller and callee pairs they
  make, and the sites that bound nothing by reason — an import of code outside the repository, an import that exports
  no callable of the name, a name no definition has, definitions the call cannot bind, and a choice nothing settles —
  listing the first of them in path order up to a limit and counting the rest. It resolves every site, in parallel.

Status: the API is new and is not published yet.
