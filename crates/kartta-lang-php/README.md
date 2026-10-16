# kartta-lang-php

PHP for the kartta code graph: the classes, interfaces, traits, enums, functions and methods of PHP source, the calls
between them, and the files a file includes or names in a `use`.

`Php` implements the `kartta_lang::Language` contract over the `tree-sitter-php` grammar. It claims every file whose
extension is `php` or `phtml`, in any case.

## What a file extracts to

A function, a method, a type, a property, a constant, a promoted constructor parameter, an enum case and a closure
assigned to a variable are symbols, qualified by the namespace in effect: `App\Service` for a class and
`App\Service::greet` for its method, whose owner is `Service`. A class takes the class it extends, the interfaces it
implements and the traits it `use`s as parents. A member is exported unless it is `private`. A call records its
receiver: the variable of a member call, the scope of a static call, by the last segment of a qualified one, and the
empty receiver where that is an expression. A construction calls the class by the last segment of its name.

## Resolution

An `include` or `require` whose path folds to a constant, from strings, `__DIR__`, `__FILE__`, `.` and `dirname`,
includes the file it names. A `use` includes the file that the PSR-4 autoload of the repository's `composer.json`
files maps it to, or else, for a `use function` or a `use const`, the file it maps the namespace to; any other `use` is
an import as written. A file whose name ends in `Test.php`, or one under `test` or `tests`, is test code; a
`*.generated.php` or `*_pb.php` file, or one whose leading comments carry `@generated` or `DO NOT EDIT`, is generated
code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
