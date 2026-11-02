use kartta_lang::path::{base, dir, has_component, join, sole};
use kartta_lang::{Extraction, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags};

const SOURCE_EXTENSIONS: [&str; 3] = ["", ".sh", ".bash"];

pub(crate) struct BashResolver;

impl Resolver for BashResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        Sources { repository }.resolve(raw, path)
    }
}

struct Sources<'r> {
    repository: &'r dyn Repository,
}

impl Sources<'_> {
    // constraint: a relative specifier resolves against the sourcing file's directory; any other resolves from the
    // root, and failing that to the one file whose path ends in it at a segment, counted across every extension
    // probed, so an ambiguous name stays unresolved
    fn source(&self, spec: &str, path: &str) -> Option<String> {
        if spec.starts_with("./") || spec.starts_with("../") {
            return self.probe(&join(dir(path), spec)?);
        }

        self.probe(spec).or_else(|| self.segment_suffix(spec))
    }

    fn probe(&self, stem: &str) -> Option<String> {
        SOURCE_EXTENSIONS
            .iter()
            .map(|extension| format!("{stem}{extension}"))
            .find(|candidate| self.repository.contains(candidate))
    }

    fn segment_suffix(&self, spec: &str) -> Option<String> {
        sole(SOURCE_EXTENSIONS.iter().flat_map(|extension| {
            let suffix = format!("/{spec}{extension}");
            self.repository
                .files_named(base(&suffix))
                .filter(move |candidate| candidate.ends_with(&suffix))
        }))
    }
}

impl Sources<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        if is_test_file(path) {
            extraction.flag_symbols(SymbolFlags::empty().with_test(true));
        }

        for directive in directives {
            if let Some(target) = self.source(&directive.spec, path) {
                extraction.includes.push(IncludeRef::new(target));
            }
        }

        extraction
    }
}

fn is_test_file(path: &str) -> bool {
    let name = base(path);
    name.ends_with("_test.sh")
        || name.ends_with("_test.bash")
        || name.starts_with("test_")
        || has_component(path, &["test", "tests"])
}
