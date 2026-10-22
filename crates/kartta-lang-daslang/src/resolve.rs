use std::iter;

use kartta_lang::path::{base, dir, forward_slashes, join, sole};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

pub(crate) struct DaslangResolver;

impl Resolver for DaslangResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        Modules { repository }.resolve(raw, path)
    }
}

struct Modules<'r> {
    repository: &'r dyn Repository,
}

impl Modules<'_> {
    // constraint: the compiler binds a `require` to the module a file declares by name, matching letter case, and the
    // module path it searches is a compiler input no repository holds; a module almost always declares its own file
    // stem, so the file names stand in for the module index
    fn target(&self, spec: &str, path: &str) -> Option<String> {
        let spec = forward_slashes(spec);
        if spec.starts_with("./") || spec.starts_with("../") {
            return self.probe(&join(dir(path), &spec)?);
        }

        let stem = spec.strip_suffix(".das").unwrap_or(&spec);
        if !stem.contains(['/', '.']) {
            return self.unique_named(&format!("{stem}.das"));
        }

        let module_path = stem.replace('.', "/");
        self.nearest_ancestor(&module_path, path)
            .or_else(|| self.unique_suffix(&module_path))
            .or_else(|| self.unique_named(&format!("{}.das", base(&module_path))))
    }

    fn nearest_ancestor(&self, module_path: &str, path: &str) -> Option<String> {
        let mut ancestors =
            iter::successors(Some(dir(path)), |&above| (above != ".").then(|| dir(above)));
        ancestors.find_map(|ancestor| {
            if ancestor == "." {
                self.probe(module_path)
            } else {
                self.probe(&format!("{ancestor}/{module_path}"))
            }
        })
    }

    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "the compiler opens a module by the name as written, which matches a file's case"
    )]
    fn probe(&self, candidate: &str) -> Option<String> {
        if self.repository.contains(candidate) {
            return Some(candidate.to_owned());
        }
        if candidate.ends_with(".das") {
            return None;
        }

        let with_extension = format!("{candidate}.das");
        self.repository
            .contains(&with_extension)
            .then_some(with_extension)
    }

    fn unique_named(&self, name: &str) -> Option<String> {
        sole(self.repository.files_named(name))
    }

    fn unique_suffix(&self, module_path: &str) -> Option<String> {
        let tail = format!("{module_path}.das");
        let nested = format!("/{tail}");
        sole(
            self.repository
                .files_named(base(&tail))
                .filter(|found| *found == tail || found.ends_with(&nested)),
        )
    }
}

impl Modules<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_path(path))
            .with_generated(is_generated_name(base(path)));
        extraction.flag_symbols(path_flags);

        for directive in directives {
            match self.target(&directive.spec, path) {
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

fn is_test_path(path: &str) -> bool {
    let name = base(path);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);

    path.split('/')
        .any(|component| matches!(component, "test" | "tests"))
        || stem.ends_with("_test")
        || stem.starts_with("test_")
}

fn is_generated_name(name: &str) -> bool {
    name.contains(".gen.") || name.contains("_gen.")
}
