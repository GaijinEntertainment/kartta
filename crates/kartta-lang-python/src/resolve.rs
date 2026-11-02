use std::iter;

use kartta_lang::path::{base, child, dir, has_component, join};
use kartta_lang::{
    Extraction, ImportBinding, ImportRef, IncludeRef, RawDirective, RawExtraction, Repository,
    Resolver, SymbolFlags,
};

use crate::walk::{FUTURE_FORM, IMPORT_FORM, MODULE_FORM};

pub(crate) struct Modules;

impl Resolver for Modules {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
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
            repository,
            path,
            includes: Vec::new(),
        };
        for mut directive in directives {
            let found = imports.includes.len();
            let imported = match directive.form {
                IMPORT_FORM => directive.names.len() / 2,
                MODULE_FORM => 0,
                _ => directive.names.len(),
            };
            if directive.form != FUTURE_FORM {
                imports.resolve(&directive.spec, &directive.names[..imported]);
            }
            if imports.includes.len() > found {
                continue;
            }
            if directive.form != FUTURE_FORM {
                imports.bind_outside(&mut extraction.bindings, &directive, imported);
            }
            directive.names.truncate(imported);
            extraction.imports.push(ImportRef::new(
                directive.spec,
                directive.names,
                directive.line,
            ));
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

    // constraint: a name an absolute import of a module the repository does not hold binds holds that module's code,
    // so a call through it or of it binds no definition of the repository, as `json.dumps()` after `import json`; a
    // relative import names the package's own code, a wildcard import binds no name it writes, and `import a.b`
    // binds `a`, which the repository can hold where it does not hold `a.b`; a module file of the repository that
    // no anchor reaches, or a submodule file of an imported name, is most often reached through a `sys.path` entry,
    // so its import binds nothing
    // shortcut: a package that only its `__init__.py` holds is not looked for past the anchors, since every
    // initializer of the tree shares that name and the probe would read each of them per import; look for it once a
    // resolver keeps the tree's package directories
    fn bind_outside(
        &self,
        bindings: &mut Vec<ImportBinding>,
        directive: &RawDirective,
        imported: usize,
    ) {
        let spec = directive.spec.as_str();
        let module = dotted_path(spec);
        let (names, aliases) = directive.names.split_at(imported);
        if spec.starts_with('.')
            || module.is_empty()
            || self.holds_elsewhere(&module)
            || (names.iter()).any(|name| self.holds_elsewhere(&child(&module, name)))
        {
            return;
        }
        let last_segment = base(&module);
        if directive.form == MODULE_FORM {
            let alias = directive.names.first().cloned();
            let head = module.split('/').next().unwrap_or(&module);
            if alias.is_none()
                && head.len() < module.len()
                && (self.holds(head) || self.holds_elsewhere(head))
            {
                return;
            }
            let last_segment = if alias.is_some() { last_segment } else { head }.to_owned();
            bindings.push(ImportBinding::new(alias, None, last_segment));
            return;
        }
        bindings.extend(names.iter().zip(aliases).map(|(name, alias)| {
            ImportBinding::new(
                Some(if alias.is_empty() { name } else { alias }.clone()),
                None,
                last_segment.to_owned(),
            )
        }));
    }

    fn holds(&self, module: &str) -> bool {
        anchors(self.path).any(|anchor| self.probe(&child(anchor, module)).is_some())
    }

    fn holds_elsewhere(&self, module: &str) -> bool {
        [".py", ".pyi"].into_iter().any(|extension| {
            let file = format!("{module}{extension}");
            self.repository.files_named(base(&file)).any(|found| {
                found
                    .strip_suffix(&file)
                    .is_some_and(|dir| dir.is_empty() || dir.ends_with('/'))
            })
        })
    }

    // constraint: an absolute module is probed from the repository root first, then from each directory around the
    // importing file, nearest first, as a src layout places it; with `app/db.py` and `src/app/db.py` both present,
    // `import app.db` from under `src/` takes the root one
    fn absolute(&mut self, module: &str) -> Found {
        for anchor in anchors(self.path) {
            if let Some((target, package)) = self.probe(&child(anchor, module)) {
                self.includes.push(IncludeRef::new(target));
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
            self.includes.push(IncludeRef::new(target));
        }
        if !plain_module {
            self.submodules(&package, names);
        }
    }

    // constraint: an imported name that is no file under the package is a symbol of it and adds no include
    fn submodules(&mut self, package: &str, names: &[String]) {
        for name in names {
            if let Some((target, _)) = self.probe(&child(package, name)) {
                self.includes.push(IncludeRef::new(target));
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
        || has_component(path, &["test", "tests"])
}

fn is_generated_path(path: &str) -> bool {
    let stem = stem(base(path));
    stem.ends_with("_pb2") || stem.ends_with("_pb2_grpc")
}
