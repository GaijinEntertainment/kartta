//! Python for the kartta code graph: the functions, classes, fields, calls and imports of Python source.
//!
//! [`Python`] implements [`Language`] over the `tree-sitter-python` grammar. It claims every file whose extension is
//! `py` or `pyi`, in any case.
//!
//! # What a file extracts to
//!
//! Every function, method and class is a symbol, and so is a variable assigned to a bare name at module level, the
//! first name of a chain alone: `a = b = 1` defines `a`, and `a, b = 1, 2` defines nothing. A name assigned in a class
//! body, `name: str = ""` or `count = 0`, is a field of its class, and so is an attribute a method assigns on its first
//! parameter, `self.size = size`, declared once where it is first assigned unless the class body declares a member of
//! its name; a static method has no such parameter. A symbol is qualified by the module its path names and by the
//! classes around it: `pkg.service.Service.greet` for a method of `Service` in `pkg/service.py`, and `pkg.Service` for
//! a class in `pkg/__init__.py`; a root `__init__.py` names no module. A definition nested in a function is qualified
//! by the module alone, is no member of a class around that function, and is local to the function's body. A decorated
//! definition starts at its first decorator. A name that starts with `_` is not exported.
//!
//! Inside a definition, a class body included, and outside every definition, at file scope, each read, write (`=`)
//! and update (`+=`) of an attribute or a module-level value is a data site; where the module binds a name is no site
//! of it. An attribute on a method's first parameter is a member of the class around the method. One on a name that
//! neither a function around it nor the class body it is read in binds, or that a `global` declaration gives the
//! module, takes that name as its owner path where the module binds it to a class, a `def` or an import, or binds it
//! not at all, and binds only a member of an owner of that name. On anything else, a value the module binds, a name
//! those scopes bind or an expression that is no name, the path is empty, a member of a value of no known type, which
//! binds the one field of the name the file declares. A name that no function around it binds, or that a `global`
//! declaration gives the module, uses the module's name wherever the module binds it, by an import, an assignment, a
//! `def` or a `class`, since a body runs once the module has bound its names. A class body's read or update of a name
//! that body binds is a member of the class; the name reaches no method, nested class, lambda or comprehension past
//! its first iterable.
//!
//! A call records its callee and, for a member call, its receiver: `self.greet()` records `greet` on `self`, and a
//! member call on anything but a name, `items[0].close()`, records the empty receiver. A call is attributed to the
//! function around it, and a call in a class body outside every method, a lambda's, a comprehension's and a method
//! decorator's included, or in the class's bases, to the class; a class with no name, which only a broken parse
//! leaves, keeps the caller around it. A name in the bases is looked up in the scope around the class, where Python
//! evaluates them. A call in a default argument, `def f(x=make()):`, is attributed to `f`, and its name is looked up
//! in the scope of `f`. A call outside every function and class is a call from file scope, before the file's first
//! definition too.
//!
//! A bare call of a function or class that a function around the call defines is a local call to that definition, as
//! Python's scoping finds the name, and a bare call of a value that such a function binds, a parameter or the target
//! of an assignment, a loop, a `with` or an `except`, is not recorded, because nothing tells what the value holds. Nor
//! is a bare call of a builtin's name, `list()`, that neither a scope around it nor the module binds, by a definition,
//! an import or an assignment, because it reaches the builtin and no function of another file. An
//! import or a `global` declaration nearer the call hides a definition around it, a `nonlocal` declaration does not,
//! and the names a class body binds reach a call in that body but neither its methods nor the function around the
//! class.
//!
//! # Resolution
//!
//! An `import` or `from ... import` that names a module of the repository includes its file, `a/b.py` or
//! `a/b/__init__.py`, or failing both its stub, `a/b.pyi` or `a/b/__init__.pyi`: an absolute module is probed from the
//! repository root and then from each directory around the importing file, nearest first, and a relative one from the
//! importing file's directory, one level up for each leading dot after the first. A from-import also includes each
//! imported name that is a submodule of the package, a namespace package without an `__init__.py` included, and probes
//! none below a plain module file. An import that includes no file, such as one of the standard library or a
//! `from __future__ import`, is recorded as written. An absolute one also binds the names it writes outside the
//! repository, `json` for `import json`, `np` for `import numpy as np` and `Field` for
//! `from sphinx.util.docfields import Field`, so a call through such a name or of it binds no definition of the
//! repository. It binds none where the repository holds the module's own file, `a/b.py` or `a/b.pyi`, or that of a
//! name the import writes as a submodule, at any depth, as one a `sys.path` entry reaches; a package that only its
//! `__init__.py` holds counts there only where the probe reaches it. `import a.b` binds no `a` that the repository
//! holds.
//!
//! A member call on `self`, `cls` or `other`, or on a class the module defines or imports, binds the member the nearest
//! class above that class holds, where the class holds none itself. A parent is read by the last name its base writes,
//! among the classes of the module and of the files its imports reach, unless the name the base starts with comes from
//! outside the repository.
//!
//! A file named `test_*`, `*_test` or `conftest`, or below a `test` or `tests` directory in any case, is test code. A
//! `*_pb2` or `*_pb2_grpc` file, or one with a `@generated` or `DO NOT EDIT` marker in the comments and module
//! docstring that open it, outside a backtick span, is generated code. The header is read after a UTF-8 byte order
//! mark, and its docstring is a triple-quoted plain, raw or unicode string, never a bytes or formatted one, that no
//! escaped quote closes. A `.pyi` stub takes the module name and the role
//! of the `.py` source it describes. A free `def main` at module level is an entry point, as is a method whose name
//! opens and ends with `__`, the spelling Python keeps for the special methods it calls, `__init__` or `__eq__`, and a
//! method that `property` or `cached_property` decorates or that an accessor of the property of its name does,
//! `@size.setter` over `def size`: no call spells their names. The spelling is a heuristic, so a class's own
//! `__custom__` is one too.
//!
//! # Fingerprint
//!
//! [`Python::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_python::Python;
//!
//! let python = Python::new();
//! assert!(python.claims("pkg/service.py"));
//!
//! let raw = python.extract(b"class Service:\n    def greet(self):\n        return helper()\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["Service", "Service.greet"]);
//! assert_eq!(raw.calls[0].callee, "helper");
//! ```

use kartta_lang::path::has_extension;
use kartta_lang::{
    Conventions, DocComments, Language, OwnMembers, RawExtraction, ReceiverMembers, Repository,
    Resolver, ValueUses,
};

use crate::resolve::Modules;

mod banner;
mod resolve;
mod runtime;
mod walk;

/// The Python language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Python;

impl Python {
    /// The Python language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    conventions.owner_chain_separator = Some(".");
    conventions.runtime_methods = runtime::METHODS;
    conventions.value_uses = ValueUses::Imported;
    conventions.own_type_receivers = Some(&["cls", "other", "self"]);
    conventions.receiver_members = ReceiverMembers::InheritedUpToAnyField;
    conventions.constructor = Some("__init__");
    conventions
};

impl Language for Python {
    fn name(&self) -> &'static str {
        "python"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["py", "pyi"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_python::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::lines(&["#"])
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(Modules)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

const fn is_python_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("py") || extension.eq_ignore_ascii_case("pyi")
}
