# kartta-lang-puppet

Puppet for the kartta code graph: the classes, defined types, nodes and functions of Puppet manifests, and the class
references between them.

`Puppet` implements the `kartta_lang::Language` contract over the `tree-sitter-puppet` grammar. It claims every file
whose extension is `pp`, in any case.

## What a file extracts to

A class, a defined type, a node definition, a function and a type alias are symbols, named as written and exported; a
class takes the class it `inherits` as its parent. A statement inside a class, a defined type, a node or a function is
a call made by it, and a statement outside them all a call made at file scope. A resource declaration is a call of its
type, a function call a call of its name, and a call on a value records the value as its receiver.

A class reference — an `include`, `require` or `contain` statement with a bare or a quoted name, a `contain(...)` call,
a resource of a qualified type, and the class a class `inherits` — is a call of the class or defined type itself. A
reference that names no class, such as `Class['foo']` or a `$variable`, records nothing, and neither does
`include(...)` or `require(...)`, which the grammar does not parse.

## Resolution

A class reference includes the manifest the module autoloader loads for it: `foo::bar::baz` names
`foo/manifests/bar/baz.pp` at the root or under `modules/`, `site/`, `site-modules/` or `dist/`, else the one file
anywhere whose path ends that way. A file under a `spec`, `tests`, `test` or `examples` directory beside a module's
`manifests/`, each in the lower case of the module layout, is test code, and one whose leading comments carry
`@generated` or `DO NOT EDIT` is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
