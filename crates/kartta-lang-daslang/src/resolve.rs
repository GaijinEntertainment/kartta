use std::borrow::Cow;

use kartta_lang::path::{base, dir, join};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

pub(crate) struct DaslangResolver<'r> {
    repository: &'r dyn Repository,
}

impl<'r> DaslangResolver<'r> {
    pub(crate) const fn new(repository: &'r dyn Repository) -> Self {
        Self { repository }
    }

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
        let mut ancestor = dir(path);
        loop {
            let candidate = if ancestor == "." {
                module_path.to_owned()
            } else {
                format!("{ancestor}/{module_path}")
            };
            if let Some(found) = self.probe(&candidate) {
                return Some(found);
            }
            if ancestor == "." {
                return None;
            }
            ancestor = dir(ancestor);
        }
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

impl Resolver for DaslangResolver<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_path(path))
            .with_generated(is_generated_name(base(path)));
        if path_flags != SymbolFlags::empty() {
            for symbol in &mut extraction.symbols {
                symbol.flags = symbol.flags.union(path_flags);
            }
        }

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

fn forward_slashes(spec: &str) -> Cow<'_, str> {
    if spec.contains('\\') {
        Cow::Owned(spec.replace('\\', "/"))
    } else {
        Cow::Borrowed(spec)
    }
}

fn sole(mut found: impl Iterator<Item = String>) -> Option<String> {
    let first = found.next()?;
    found.next().is_none().then_some(first)
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
