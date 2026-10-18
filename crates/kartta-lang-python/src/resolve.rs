use std::iter;

use kartta_lang::path::{base, child, dir, join};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

use crate::walk::IMPORT_FORM;

pub(crate) struct Modules<'r> {
    repository: &'r dyn Repository,
}

impl<'r> Modules<'r> {
    pub(crate) const fn new(repository: &'r dyn Repository) -> Self {
        Self { repository }
    }
}

impl Resolver for Modules<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let module = module_name(path);
        let flags = SymbolFlags::empty()
            .with_test(is_test_file(path))
            .with_generated(is_generated_path(path));
        for symbol in &mut extraction.symbols {
            if !module.is_empty() {
                symbol.qualified_name = format!("{module}.{}", symbol.qualified_name);
            }
            symbol.flags = symbol.flags.union(flags);
        }

        let mut imports = Imports {
            repository: self.repository,
            path,
            includes: Vec::new(),
        };
        for directive in directives {
            let found = imports.includes.len();
            if directive.form == IMPORT_FORM {
                imports.resolve(&directive.spec, &directive.names);
            }
            if imports.includes.len() > found {
                continue;
            }
            extraction.imports.push(ImportRef {
                module: directive.spec,
                names: directive.names,
                line: directive.line,
            });
        }
        extraction.includes = imports.includes;

        extraction
    }
}

struct Imports<'a> {
    repository: &'a dyn Repository,
    path: &'a str,
    includes: Vec<IncludeRef>,
}

impl Imports<'_> {
    // constraint: a from-import without names and a wildcard one resolve like a plain import, to the module alone
    fn resolve(&mut self, spec: &str, names: &[String]) {
        if spec.starts_with('.') {
            self.relative(spec, names);
            return;
        }
        let module = dotted_path(spec);
        if module.is_empty() {
            return;
        }
        match self.absolute(&module) {
            Found::Package(package) => self.submodules(&package, names),
            Found::Module => {}
            Found::Nowhere => self.namespace(&module, names),
        }
    }

    // constraint: an absolute module is probed from the repository root first, then from each directory around the
    // importing file, nearest first, as a src layout places it; with `app/db.py` and `src/app/db.py` both present,
    // `import app.db` from under `src/` takes the root one
    fn absolute(&mut self, module: &str) -> Found {
        for anchor in anchors(self.path) {
            if let Some((target, package)) = self.probe(&child(anchor, module)) {
                self.includes.push(IncludeRef { target });
                return package.map_or(Found::Module, Found::Package);
            }
        }
        Found::Nowhere
    }

    // constraint: a module found nowhere may be a namespace package, a directory without an `__init__.py`; its names
    // are probed under the anchors in the order a module is, and the first anchor that holds one of them is the
    // package, so a portion of it under a later anchor is not merged
    fn namespace(&mut self, module: &str, names: &[String]) {
        if names.is_empty() {
            return;
        }
        for anchor in anchors(self.path) {
            let found = self.includes.len();
            self.submodules(&child(anchor, module), names);
            if self.includes.len() > found {
                return;
            }
        }
    }

    // constraint: a relative import climbs one directory per leading dot after the first, and the module it names is a
    // dependency whether or not its names are submodules; its names are probed as submodules unless it is a plain
    // module file, because a directory without an `__init__.py` is still a namespace package
    fn relative(&mut self, spec: &str, names: &[String]) {
        let dots = spec.bytes().take_while(|&byte| byte == b'.').count();
        let Some(climbed) = join(dir(self.path), &"../".repeat(dots - 1)) else {
            return;
        };
        let package = child(&climbed, &dotted_path(&spec[dots..]));

        let mut plain_module = false;
        if let Some((target, as_package)) = self.probe(&package) {
            plain_module = as_package.is_none();
            self.includes.push(IncludeRef { target });
        }
        if !plain_module {
            self.submodules(&package, names);
        }
    }

    // constraint: an imported name that is no file under the package is a symbol of it and adds no include
    fn submodules(&mut self, package: &str, names: &[String]) {
        for name in names {
            if let Some((target, _)) = self.probe(&child(package, name)) {
                self.includes.push(IncludeRef { target });
            }
        }
    }

    // constraint: a module is its `.py` file or its package's `__init__.py`, and failing both its stub, `.pyi` or
    // `__init__.pyi`, which a repository that ships only the types of a module holds
    fn probe(&self, module: &str) -> Option<(String, Option<String>)> {
        for (extension, initializer) in [(".py", "__init__.py"), (".pyi", "__init__.pyi")] {
            let file = format!("{module}{extension}");
            if self.repository.contains(&file) {
                return Some((file, None));
            }
            let init = child(module, initializer);
            if self.repository.contains(&init) {
                return Some((init, Some(module.to_owned())));
            }
        }
        None
    }
}

enum Found {
    Module,
    Package(String),
    Nowhere,
}

fn anchors(path: &str) -> impl Iterator<Item = &str> {
    let around = iter::successors(Some(dir(path)).filter(|from| *from != "."), |from| {
        from.rsplit_once('/').map(|(parent, _)| parent)
    });
    iter::once("").chain(around)
}

fn dotted_path(dotted: &str) -> String {
    let mut path = String::with_capacity(dotted.len());
    for segment in dotted.split('.').filter(|segment| !segment.is_empty()) {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);
    }
    path
}

// constraint: a `.pyi` stub takes the module name and the role of the `.py` source it describes; the extension
// matches in any case, as the claim does
fn stem(name: &str) -> &str {
    name.rsplit_once('.')
        .filter(|(_, extension)| crate::is_python_extension(extension))
        .map_or(name, |(stem, _)| stem)
}

// constraint: a package's `__init__` is the module of its directory, so `pkg/__init__.py` names `pkg` and a root
// `__init__.py` names no module
fn module_name(path: &str) -> String {
    let module = stem(path);
    let module = match module.strip_suffix("__init__") {
        Some("") => "",
        Some(package) => package.strip_suffix('/').unwrap_or(module),
        None => module,
    };
    module.replace('/', ".")
}

fn is_test_file(path: &str) -> bool {
    let stem = stem(base(path));
    stem == "conftest"
        || stem.ends_with("_test")
        || stem.starts_with("test_")
        || path
            .split('/')
            .any(|component| matches!(component, "test" | "tests"))
}

fn is_generated_path(path: &str) -> bool {
    let stem = stem(base(path));
    stem.ends_with("_pb2") || stem.ends_with("_pb2_grpc")
}
