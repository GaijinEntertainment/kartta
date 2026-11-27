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
A call belongs to the method around it, and outside every method to the class or the module whose body holds it. A call
records its receiver: a plain name as written, `self` as the caller's own type, a literal as the class of its value,
`String` for `"a"`, and any other expression as the empty receiver; a type's body runs with `self` as the type, so a
call there with no receiver records the type. A receiver that is a class, `self` in a class method or a type's body,
`self.class` in an instance method, or a constant, ends in `.`, the separator of a class method's name: `Geo::Shape.`
for `self`, a constant's path as written, `Shape.`, and for `self.class` the owner from the root, `::Geo::Shape.`, or
`.` alone in a module's method or a class written as a path, so a call on the class binds `Shape.build` before
`Shape#build`, and any other receiver the reverse. A call on a literal, or on a constant the tree
holds no value of, binds only a method of the class it names or of one that class derives from, so `Hash.new` binds
nothing where the tree defines no `Hash`. A write to a scoped constant, `Foo::BAR = 5`, defines it on the module its
path names, and a call on it binds through the class its value holds, `Foo` for `A::B = Foo.new`, and on any other value
only as a guess. A bare `new` that reads no local makes, in a class method, an instance of the owner, so `new.getopts`
there records `Geo::Shape`. A bare name, `setup`, is a call with no receiver unless a binding before it in its scope
makes it a local variable, as Ruby decides. The conventions list `other` alone as the receiver of a value of the
caller's own type, since a call on `self` binds through the type it records, so `path.to_s` inside `Keg#to_s` binds no
member of the caller's own class. A method named by a symbol, `method(:render)` or `send(:render)`, is a use of the
method of the object's own type, or of a class's for a constant receiver, `Cache.send(:flush)` or
`Cache::Store.send(:flush)`, its class method of the name first. At a site no class owns, the symbol names a top-level
method, a private method of `Object` that every object reaches.

## Resolution

A `require_relative` resolves from the requiring file's directory, and a `require` from the first of `lib`, the root,
`test`, `spec` and `app` that holds it. A required file of the repository is an include, and any other require an
import as written; a require whose string holds an interpolation or an escape names nothing. A file under a `lib`
directory belongs to the library the directory above its outermost `lib` roots, a gem or a vendored copy of the
standard library, and a file under no `lib` to the library above its outermost `test`, `spec` or `app` directory, so a
call on an owner binds the owner of its own copy first. A `*_test.rb` or
`*_spec.rb` file, or one under `test`, `tests` or `spec` in any case, is test code; a `*_pb.rb` file, or one whose
leading comments carry `@generated`, `DO NOT EDIT` or `This file is auto-generated`, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
