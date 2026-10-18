use kartta_lang::{Extraction, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags};

use crate::class::manifest;
use crate::walk::CLASS_FORM;

pub(crate) struct Manifests<'r> {
    pub(crate) repository: &'r dyn Repository,
}

impl Manifests<'_> {
    // constraint: a `spec`, `tests`, `test` or `examples` directory is test code only at a module's root, beside
    // `manifests/`; under `manifests/` the same name is a code namespace, `mymod::test::foo`, a directory that holds
    // `manifests/` itself is a module of that name, and one with no `manifests/` beside it holds modules, as an
    // environment does
    fn is_test_file(&self, path: &str) -> bool {
        let mut start = 0;
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            let module = &path[..start];
            start += component.len() + 1;
            match component {
                "manifests" => return false,
                "spec" | "tests" | "test" | "examples"
                    if components.peek() != Some(&"manifests") && self.holds_manifests(module) =>
                {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn holds_manifests(&self, module: &str) -> bool {
        let manifests = format!("{module}manifests");
        self.repository.files_in_dir(&manifests).next().is_some()
    }
}

impl Resolver for Manifests<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        if self.is_test_file(path) {
            let test = SymbolFlags::empty().with_test(true);
            for symbol in &mut extraction.symbols {
                symbol.flags = symbol.flags.union(test);
            }
        }

        let manifests = directives
            .iter()
            .filter(|directive| directive.form == CLASS_FORM)
            .filter_map(|directive| manifest(&directive.spec, self.repository));
        extraction
            .includes
            .extend(manifests.map(|target| IncludeRef { target }));

        extraction
    }
}
