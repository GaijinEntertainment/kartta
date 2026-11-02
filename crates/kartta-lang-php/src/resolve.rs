use kartta_lang::path::{base, dir, has_component, join, sole};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

use crate::fold::{MARKER, substitute_markers};
use crate::psr4::Psr4;
use crate::walk::{INCLUDE_FORM, USE_FORM};

pub(crate) struct Autoload {
    psr4: Psr4,
}

impl Autoload {
    pub(crate) fn new(repository: &dyn Repository) -> Self {
        Self {
            psr4: Psr4::read(repository),
        }
    }

    // constraint: an explicit `./` or `../` path and one under the file's own directory, which `__DIR__` folds to,
    // name a file from there; any other path names one from the root, or else the one file whose path ends in it
    fn included_file(spec: &str, path: &str, repository: &dyn Repository) -> Option<String> {
        let here = dir(path);
        let contained = |file: &String| repository.contains(file);

        if spec.starts_with("./") || spec.starts_with("../") {
            return join(here, spec).filter(contained);
        }
        if spec
            .strip_prefix(here)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        {
            return join("", spec).filter(contained);
        }
        if repository.contains(spec) {
            return Some(spec.to_owned());
        }

        let suffix = format!("/{spec}");
        sole(
            repository
                .files_named(base(spec))
                .filter(|candidate| candidate.ends_with(&suffix)),
        )
    }

    fn used_file(&self, form: u32, name: &str, repository: &dyn Repository) -> Option<String> {
        if form == USE_FORM {
            self.psr4.class_file(name, repository)
        } else {
            self.psr4.function_or_const_file(name, repository)
        }
    }
}

impl Resolver for Autoload {
    // constraint: a `use` that PSR-4 maps to a file of the repository is an include, and any other `use` an import as
    // written; an `include` names a path, which is an include or nothing
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_file(path))
            .with_generated(is_generated_file(path));
        extraction.flag_symbols(path_flags);

        for directive in directives {
            if directive.form != INCLUDE_FORM {
                match self.used_file(directive.form, &directive.spec, repository) {
                    Some(target) => extraction.includes.push(IncludeRef::new(target)),
                    None => extraction.imports.push(ImportRef::new(
                        directive.spec,
                        Vec::new(),
                        directive.line,
                    )),
                }
                continue;
            }

            let spec = if directive.spec.contains(MARKER) {
                substitute_markers(&directive.spec, path)
            } else {
                directive.spec
            };
            let included = Self::included_file(&spec, path, repository);
            extraction.includes.extend(included.map(IncludeRef::new));
        }

        extraction
    }
}

// constraint: PHPUnit finds a test by the `Test.php` ending of its file's name, case-sensitive, so neither `latest.php`
// nor `FooTest.PHP` nor a `FooTest.phtml` template is a test
fn is_test_file(path: &str) -> bool {
    base(path).ends_with("Test.php") || has_component(path, &["test", "tests"])
}

fn is_generated_file(path: &str) -> bool {
    let name = base(path);
    name.ends_with(".generated.php") || name.ends_with("_pb.php")
}
