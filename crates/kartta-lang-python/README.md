# kartta-lang-python

Python for the kartta code graph: the functions, classes, fields, calls and imports of Python source.

`Python` implements the `kartta_lang::Language` contract over the `tree-sitter-python` grammar. It claims every file
whose extension is `py` or `pyi`, in any case.

## What a file extracts to

Every function, method and class is a symbol, and so is a variable assigned to a bare name at module level. A bare
name assigned in a class body is a field of its class, and so is an attribute a method assigns on its first parameter,
`self.size = size`, declared once where it is first assigned unless the class body declares a member of its name; a
static method has no such parameter. A symbol is qualified by the module its path names and by the classes around it:
`pkg.service.Service.greet` for a method of `Service` in `pkg/service.py`, and
`pkg.Service` for a class in `pkg/__init__.py`. A decorated definition starts at its first decorator, and a name that
starts with `_` is not exported.

Inside a definition, a class body included, and outside every definition, at file scope, each read, write and update
of an attribute or a module-level value is a data site; where the module binds a name is no site of it. An attribute on
a method's first parameter binds the class around the method, or a base found without a guess. One on a name that
neither a function around it nor the class body it is read in binds, or that a `global` declaration gives the module,
takes that name as its owner path where the module binds it to a class, a `def` or an import, or binds it not at all,
and binds only a member of an owner of that name. On anything else, a value the module binds, a name those scopes bind
or an expression that is no name, the path is empty, and the attribute binds the one field of the name the file
declares. A name that no function around it binds, or that a `global` declaration gives the module, uses the module's
name wherever the module binds it, by an import, an assignment, a `def` or a `class`, and binds the declaration in the
file or the one imported module that declares it. A class body's read or update of a name that body binds is a member
of the class; the name reaches no method, nested class, lambda or comprehension past its first iterable.

A call records its callee and, for a member call, its receiver: `self.greet()` records `greet` on `self`, and a member
call on anything but a name records the empty receiver. A call belongs to the function around it, and one in a class
body outside every method, or in the class's bases, to the class; a name in the bases is looked up in the scope around
the class. A bare call of a function or class that a function around the call defines is a local call to that
definition, unless a name nearer the call hides it, as Python's scoping decides, and a bare call of a value such a
function binds, such as a parameter, is not recorded. Nor is a bare call of a builtin's name, `list()`, that neither a
scope around it nor the module binds, because it reaches the builtin; the walk does not infer the names a wildcard
import binds. The conventions name the methods of the built-in types and of the standard library's values a script
holds most, so a member call of one, `name.strip()` or `path.read_text()`, binds a member of the name and no free
function of the repository. They list `self`, `cls` and
`other` as the receivers of a value of the caller's own type, so a member call on another name, `sock.close()` inside a
class with a `close`, binds no member of the caller's own class.

## Resolution

An import that names a module of the repository includes its file, `a/b.py` or `a/b/__init__.py`: an absolute module is
probed from the repository root, then from each directory around the importing file; a relative one climbs one
directory for each leading dot after the first. A from-import also includes each imported name that is a submodule of
the package, a namespace package without an `__init__.py` included; a plain module file has no submodules. An import
that includes no file, such as one of the standard library, is recorded as written. An absolute one also binds the
names it writes, `json` for `import json` or `np` for `import numpy as np`, to code outside the repository, so a call
through such a name or of it binds no function of the repository. It binds none where the repository holds the
module's own file, `a/b.py` or `a/b.pyi`, or that of a name it imports as a submodule, at any depth, as one a
`sys.path` entry reaches; a package that only its `__init__.py` holds counts only where the probe reaches it.

A `test_*`, `*_test` or `conftest` file, or a file below a `test` or `tests` directory in any case, is test code. A
`*_pb2` or `*_pb2_grpc` file, or one with a `@generated` or `DO NOT EDIT` marker in its opening comments and module
docstring, is generated code. A free `def main` at module level is an entry point, as is a method whose name opens and
ends with `__`, the spelling Python keeps for the special methods it calls, `__init__` or `__eq__`, and a method that
`property` or `cached_property` decorates or that an accessor of the property of its name does, `@size.setter` over
`def size`: no call spells their names. The spelling is a heuristic, so a class's own `__custom__` is one too.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
