use std::borrow::Cow;
use std::iter;

use kartta_lang::path::{base, dir, forward_slashes, has_component, join, sole};
use kartta_lang::{
    Extraction, ImportBinding, ImportRef, IncludeRef, RawDirective, RawExtraction, Repository,
    Resolver, SymbolFlags,
};

use crate::walk::REQUIRE_FORM;

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
    // stem, so the file names stand in for the module index; a project roots a name and mounts its first component by
    // a program the engine does not run, so the nearest ancestor stands in for the root and the path below that
    // component for the mount
    fn target(&self, directive: &RawDirective, path: &str) -> Option<String> {
        let spec = forward_slashes(&directive.spec);
        let file = file_named(directive.form, &spec);
        if spec.starts_with("./") || spec.starts_with("../") {
            return self.probe(&join(dir(path), &file)?);
        }

        self.nearest_ancestor(&file, path)
            .or_else(|| self.unique_suffix(&file))
            .or_else(|| {
                let (_, below_mount) = file.split_once('/')?;
                self.unique_suffix(below_mount)
                    .or_else(|| self.unique_named(base(below_mount)))
            })
    }

    fn nearest_ancestor(&self, module_file: &str, path: &str) -> Option<String> {
        let mut ancestors =
            iter::successors(Some(dir(path)), |&above| (above != ".").then(|| dir(above)));
        ancestors.find_map(|ancestor| {
            if ancestor == "." {
                self.probe(module_file)
            } else {
                self.probe(&format!("{ancestor}/{module_file}"))
            }
        })
    }

    fn probe(&self, candidate: &str) -> Option<String> {
        self.repository
            .contains(candidate)
            .then(|| candidate.to_owned())
    }

    fn unique_named(&self, name: &str) -> Option<String> {
        sole(self.repository.files_named(name))
    }

    fn unique_suffix(&self, file: &str) -> Option<String> {
        let nested = format!("/{file}");
        sole(
            self.repository
                .files_named(base(file))
                .filter(|found| found == file || found.ends_with(&nested)),
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
            let target = self.target(&directive, path);
            extraction
                .re_exports
                .iter_mut()
                .filter(|re_export| re_export.source_path == directive.spec)
                .for_each(|re_export| re_export.source_file.clone_from(&target));
            if let Some(target) = target {
                extraction
                    .bindings
                    .extend(aliased(&directive, Some(&target)));
                extraction.includes.push(IncludeRef { target });
            } else {
                self.bind_outside_the_graph(&directive, &mut extraction.bindings);
                extraction.imports.push(ImportRef {
                    module: directive.spec,
                    names: Vec::new(),
                    line: directive.line,
                });
            }
        }

        extraction
    }

    // constraint: a module no file is named for lies outside the repository, most often compiled into the runtime; of
    // two files named for a module the resolver names neither, though the module is the graph's, so a file of its name
    // keeps the binding off
    fn bind_outside_the_graph(&self, directive: &RawDirective, bindings: &mut Vec<ImportBinding>) {
        if directive.form != REQUIRE_FORM {
            return;
        }
        let module = module_named(directive);
        let mut files = self.repository.files_named(&format!("{module}.das"));
        if files.next().is_none() {
            bindings.push(ImportBinding {
                alias: None,
                target_dir: None,
                last_segment: module,
            });
            bindings.extend(aliased(directive, None));
        }
    }
}

// constraint: the compiler takes an `include` as a file name as written (`src/simulate/debug_info.cpp:993`), and a
// `require` as one only where it both starts with `./`, `../` or `%/` and ends in a file's extension
// (`src/simulate/fs_file_info.cpp:253`); any other `require` is a module name, each dot a slash
// (`src/simulate/debug_info.cpp:1009`), so `x/y.das` is `x/y/das.das` and a leading `.` or `..` leaves an empty
// component under the importer's directory
fn file_named(form: u32, spec: &str) -> Cow<'_, str> {
    if form != REQUIRE_FORM || is_file_path(spec) {
        return Cow::Borrowed(spec);
    }
    let module_path = spec.trim_start_matches(['.', '/']).replace('.', "/");
    Cow::Owned(format!("{module_path}.das"))
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the compiler compares the specifier's last bytes, which matches a file's case"
)]
fn is_file_path(spec: &str) -> bool {
    (spec.ends_with(".das") || spec.ends_with(".das_project"))
        && (spec.starts_with("./") || spec.starts_with("../") || spec.starts_with("%/"))
}

fn module_named(directive: &RawDirective) -> String {
    let spec = forward_slashes(&directive.spec);
    let stem = (spec.rsplit_once('.'))
        .filter(|_| is_file_path(&spec))
        .map_or(&*spec, |(stem, _)| stem);
    let start = stem.rfind(['/', '.']).map_or(0, |separator| separator + 1);
    stem[start..].to_owned()
}

fn aliased(directive: &RawDirective, target: Option<&str>) -> Option<ImportBinding> {
    Some(ImportBinding {
        alias: Some(directive.names.first()?.clone()),
        target_dir: target.map(str::to_owned),
        last_segment: module_named(directive),
    })
}

fn is_test_path(path: &str) -> bool {
    let name = base(path);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);

    has_component(path, &["test", "tests"]) || stem.ends_with("_test") || stem.starts_with("test_")
}

fn is_generated_name(name: &str) -> bool {
    name.contains(".gen.") || name.contains("_gen.")
}
