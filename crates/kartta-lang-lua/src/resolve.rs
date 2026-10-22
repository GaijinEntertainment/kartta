use kartta_lang::path::{base, dir, join};
use kartta_lang::{
    Extraction, IncludeRef, RawDirective, RawExtraction, Repository, Resolver, SymbolFlags,
};

use crate::walk::REQUIRE_FORM;

const MODULE_FILES: [&str; 2] = [".lua", "/init.lua"];

pub(crate) struct LuaResolver;

impl Resolver for LuaResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        Loaders { repository }.resolve(raw, path)
    }
}

struct Loaders<'r> {
    repository: &'r dyn Repository,
}

impl Loaders<'_> {
    // constraint: `require` takes a dotted module, `a.b` for `a/b.lua` or the package `a/b/init.lua`, from the root
    // and then from the requiring file's directory; `dofile` and `loadfile` take a path, whose dots are no separators,
    // from the file's directory and then from the root. An empty name loads nothing.
    fn loaded_file(&self, directive: &RawDirective, path: &str) -> Option<String> {
        if directive.spec.is_empty() {
            return None;
        }

        let from = dir(path);
        let contained = |candidate: &String| self.repository.contains(candidate);
        if directive.form != REQUIRE_FORM {
            return join(from, &directive.spec).filter(contained).or_else(|| {
                self.repository
                    .contains(&directive.spec)
                    .then(|| directive.spec.clone())
            });
        }

        let module = directive.spec.replace('.', "/");
        let in_module = |module: &str| {
            MODULE_FILES
                .iter()
                .map(|file| format!("{module}{file}"))
                .find(contained)
        };
        in_module(&module).or_else(|| in_module(&join(from, &module)?))
    }
}

impl Loaders<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_file(path))
            .with_generated(is_generated_file(path));
        extraction.flag_symbols(path_flags);

        let loaded = directives
            .iter()
            .filter_map(|directive| self.loaded_file(directive, path));
        extraction
            .includes
            .extend(loaded.map(|target| IncludeRef { target }));

        extraction
    }
}

fn is_test_file(path: &str) -> bool {
    let name = base(path);
    name.ends_with("_spec.lua")
        || name.ends_with("_test.lua")
        || path
            .split('/')
            .any(|component| matches!(component, "spec" | "test" | "tests"))
}

fn is_generated_file(path: &str) -> bool {
    let name = base(path);
    name.ends_with(".gen.lua") || name.ends_with("_pb.lua")
}
