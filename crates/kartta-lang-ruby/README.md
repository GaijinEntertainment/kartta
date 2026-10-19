# kartta-lang-ruby

Ruby for the kartta code graph: the classes, modules, methods and constants of Ruby source, the calls between them, and
the files a file requires.

`Ruby` implements the `kartta_lang::Language` contract over the `tree-sitter-ruby` grammar. It claims every file whose
extension is `rb`, in any case.

## What a file extracts to

A class, a module, a method, a constant, an `attr_*` accessor and an alias are symbols, qualified by the whole namespace
chain of their type: `Geo::Shape#area` for an instance method, `Geo::Shape.build` for a class method, and
`Geo::Shape::MAX` for a constant. A class takes its superclass and its mixins as parents. A method is exported unless a
`private` modifier covers it. An alias of a method the file declares before it is one callable with that method's body.
A call records its receiver: a plain name as written, `self` as the caller's own type, and any other expression as the
empty receiver. A method named by a symbol, `method(:render)` or `send(:render)`, is a use of the method of the
object's own type, or of a class's for a constant receiver, `Cache.send(:flush)` or `Cache::Store.send(:flush)`.
At a site no class owns, the symbol names a top-level method, a private method of `Object` that every object reaches.

## Resolution

A `require_relative` resolves from the requiring file's directory, and a `require` from the first of `lib`, the root,
`test`, `spec` and `app` that holds it. A required file of the repository is an include, and any other require an
import as written; a require whose string holds an interpolation or an escape names nothing. A `*_test.rb` or
`*_spec.rb` file, or one under `test`, `tests` or `spec`, is test code; a `*_pb.rb` file, or one whose leading comments
carry `@generated`, `DO NOT EDIT` or `This file is auto-generated`, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
