use kartta_lang::path::{base, dir, join, shared_segments};
use kartta_lang::{Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver};

pub(crate) struct Includes<'r> {
    pub(crate) repository: &'r dyn Repository,
}

impl Includes<'_> {
    // constraint: the shader compiler looks an include up under the include paths of the build configuration that
    // compiles the file, which the repository does not tie to a file, so a spelling is matched against the paths
    // that end in it, and of several the one nearest the including file wins
    fn include(&self, spec: &str, path: &str) -> Option<String> {
        let spec = spec.trim_start_matches("./");
        if spec.split('/').any(|component| component == "..") {
            return join(dir(path), spec).filter(|joined| self.repository.contains(joined));
        }

        let mut nearest: Option<(usize, String)> = None;
        let mut tied = false;
        let candidates = self
            .repository
            .files_named(base(spec))
            .filter(|candidate| ends_at_segment(candidate, spec));
        for candidate in candidates {
            let nearness = shared_segments(dir(&candidate), dir(path));
            match &nearest {
                Some((best, _)) if nearness < *best => {}
                Some((best, _)) if nearness == *best => tied = true,
                _ => {
                    nearest = Some((nearness, candidate));
                    tied = false;
                }
            }
        }
        nearest.filter(|_| !tied).map(|(_, winner)| winner)
    }
}

impl Resolver for Includes<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        for directive in directives {
            match self.include(&directive.spec, path) {
                Some(target) if target != path => extraction.includes.push(IncludeRef { target }),
                Some(_) => {}
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

fn ends_at_segment(path: &str, spec: &str) -> bool {
    path == spec
        || path
            .strip_suffix(spec)
            .is_some_and(|head| head.ends_with('/'))
}
