# kartta-lang-php

PHP for the kartta code graph: the classes, interfaces, traits, enums, functions and methods of PHP source, the calls
between them, and the files a file includes or names in a `use`.

`Php` implements the `kartta_lang::Language` contract over the `tree-sitter-php` grammar. It claims every file whose
extension is `php` or `phtml`, in any case.

## What a file extracts to

A function, a method, a type, a property, a constant, a promoted constructor parameter, an enum case and a closure
assigned to a variable are symbols, qualified by the namespace in effect: `App\Service` for a class and
`App\Service::greet` for its method, whose owner is `Service`. An anonymous class is a class named by the callable that
creates it and the variable or property it is assigned to, `{testRun·job}`, which owns its members. A class takes the
class it extends, the interfaces it implements and the traits it `use`s as parents. A member is exported unless it is
`private`. A call records its receiver: the variable of a member call, or the anonymous class the caller assigned to
that variable, the scope of a static call, by the name PHP compiles a qualified one to, and the empty receiver where
that is an expression. A construction of a named class, however spelled, calls that class and `__construct` on the name
PHP compiles it to, and binds the class and the constructor it holds or inherits, or nothing for a class outside the
tree; one of an anonymous class calls that class. A static call's scope written as a bare name other than `self`,
`static` and `parent` names a class, so a static call on a class the graph does not hold, `Mockery::mock()`, binds only
through the file's `use` imports and includes, never a namesake method found by its name alone; a variable or an
expression scope is not read so.

A callable named by a string or an array is a use of what it names: a string in the callable argument of a builtin or
a WordPress hook that takes one, `usort($a, 'cmp')` or `add_action('init', 'boot')`, and an array of a class and a
method, `[$this, 'render']` or `[Cache::class, 'flush']`. PHP reads a callable string as fully qualified: `'helper'`
names the global `helper` and `'App\helper'` the one of `App`, so a string is a use only where the namespace it spells
is the file's own. A bare use binds the function its own file declares.

## Resolution

An `include` or `require` whose path folds to a constant, from strings, `__DIR__`, `__FILE__`, `.` and `dirname`,
includes the file it names. A `use` includes the file that the PSR-4 autoload of the repository's `composer.json`
files maps it to, or else, for a `use function` or a `use const`, the file it maps the namespace to; any other `use` is
an import as written. A file whose name ends in `Test.php`, or one under `test` or `tests` in any case, is test code; a
`*.generated.php` or `*_pb.php` file, or one whose leading comments carry `@generated` or `DO NOT EDIT`, is generated
code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
