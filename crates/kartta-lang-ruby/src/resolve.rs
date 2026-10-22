use kartta_lang::path::{base, dir, join};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawDirective, RawExtraction, Repository, Resolver,
    SymbolFlags,
};

use crate::walk::REQUIRE_RELATIVE_FORM;

const LOAD_ROOTS: [&str; 5] = ["lib", "", "test", "spec", "app"];

pub(crate) struct RubyResolver;

impl Resolver for RubyResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        Requires { repository }.resolve(raw, path)
    }
}

struct Requires<'r> {
    repository: &'r dyn Repository,
}

impl Requires<'_> {
    // constraint: `require_relative` names a path from the requiring file's directory; a bare `require` names one
    // under the first conventional load root that holds it. Either may leave out the `.rb`.
    fn required_file(&self, directive: &RawDirective, path: &str) -> Option<String> {
        if directive.form == REQUIRE_RELATIVE_FORM {
            return join(dir(path), &directive.spec).and_then(|base| self.probe(base));
        }

        LOAD_ROOTS.iter().find_map(|root| {
            let base = if root.is_empty() {
                directive.spec.clone()
            } else {
                format!("{root}/{}", directive.spec)
            };
            self.probe(base)
        })
    }

    fn probe(&self, base: String) -> Option<String> {
        if self.repository.contains(&base) {
            return Some(base);
        }
        let with_extension = format!("{base}.rb");
        self.repository
            .contains(&with_extension)
            .then_some(with_extension)
    }
}

impl Requires<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_file(path))
            .with_generated(base(path).ends_with("_pb.rb"));
        extraction.flag_symbols(path_flags);
        // constraint: the graph joins callables of one key across files, so the path keeps a key inside its file
        for callable in &mut extraction.callables {
            callable.key = format!("{path}#{}", callable.key);
        }

        for directive in directives {
            match self.required_file(&directive, path) {
                Some(target) => extraction.includes.push(IncludeRef { target }),
                None => extraction.imports.push(ImportRef {
                    module: directive.spec,
                    names: Vec::new(),
                    line: directive.line,
                }),
            }
        }

        extraction
    }
}

fn is_test_file(path: &str) -> bool {
    let name = base(path);
    name.ends_with("_test.rb")
        || name.ends_with("_spec.rb")
        || path
            .split('/')
            .any(|component| matches!(component, "test" | "tests" | "spec"))
}
