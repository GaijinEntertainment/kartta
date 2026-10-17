# kartta-lang-python

Python for the kartta code graph: the functions, classes, fields, calls and imports of Python source.

`Python` implements the `kartta_lang::Language` contract over the `tree-sitter-python` grammar. It claims every file
whose extension is `py` or `pyi`, in any case.

## What a file extracts to

Every function, method and class is a symbol, and so is a variable assigned to a bare name at module level and an
annotated attribute in a class body, which is a field of its class. A symbol is qualified by the module its path names
and by the classes around it: `pkg.service.Service.greet` for a method of `Service` in `pkg/service.py`, and
`pkg.Service` for a class in `pkg/__init__.py`. A decorated definition starts at its first decorator, and a name that
starts with `_` is not exported.

A call records its callee and, for a member call, its receiver: `self.greet()` records `greet` on `self`, and a member
call on anything but a name records the empty receiver. A call belongs to the function around it, one in a class body
included. A bare call of a function or class that a function around the call defines is a local call to that
definition, unless a name nearer the call hides it, as Python's scoping decides, and a bare call of a value such a
function binds, such as a parameter, is not recorded.

## Resolution

An import that names a module of the repository includes its file, `a/b.py` or `a/b/__init__.py`: an absolute module is
probed from the repository root, then from each directory around the importing file; a relative one climbs one
directory for each leading dot after the first. A from-import also includes each imported name that is a submodule of
the package, a namespace package without an `__init__.py` included; a plain module file has no submodules. An import
that includes no file, such as one of the standard library, is recorded as written.

A `test_*`, `*_test` or `conftest` file, or a file below a `test` or `tests` directory, is test code. A `*_pb2` or
`*_pb2_grpc` file, or one with a `@generated` or `DO NOT EDIT` marker in its opening comments and module docstring, is
generated code. A free `def main` at module level is an entry point, as is a method whose name opens and ends with `__`,
the spelling Python keeps for the special methods it calls, `__init__` or `__eq__`, and a method that `property` or
`cached_property` decorates or that an accessor of the property of its name does, `@size.setter` over `def size`: no
call spells their names. The spelling is a heuristic, so a class's own `__custom__` is one too.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
