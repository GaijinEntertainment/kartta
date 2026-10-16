use kartta_lang::path::{dir, join};
use kartta_lang::{
    Extraction, ImportBinding, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

pub(crate) struct GoResolver<'r> {
    repository: &'r dyn Repository,
    modules: Vec<Module>,
}

struct Module {
    prefix: String,
    dir: String,
    replaced_in: Option<String>,
}

impl<'r> GoResolver<'r> {
    pub(crate) fn new(repository: &'r dyn Repository) -> Self {
        let mut modules = Vec::new();
        for path in repository
            .files_named("go.mod")
            .filter(|path| !is_vendored(path))
        {
            if let Ok(bytes) = repository.read(&path)
                && let Ok(contents) = String::from_utf8(bytes)
            {
                read_go_mod(dir(&path), &contents, &mut modules);
            }
        }
        modules.sort_by(|a, b| {
            b.prefix
                .len()
                .cmp(&a.prefix.len())
                .then_with(|| a.prefix.cmp(&b.prefix))
        });

        Self {
            repository,
            modules,
        }
    }

    // constraint: a vendored build prefers the vendored copy; otherwise the longest module prefix decides alone, and a
    // directory under it that holds no files is not in the repository
    // constraint: the go command applies only the main module's replace directives and vendor directory, so an
    // importing file sees those of the module it belongs to
    // shortcut: a replace that names a version applies to every version of the module; telling them apart needs the
    // versions the module requires
    fn package(&self, spec: &str, owner: Option<&str>) -> Option<(String, Vec<String>)> {
        let vendored = match owner {
            None | Some("") => format!("vendor/{spec}"),
            Some(module_dir) => format!("{module_dir}/vendor/{spec}"),
        };
        let files: Vec<String> = self.repository.files_in_dir(&vendored).collect();
        if !files.is_empty() {
            return Some((vendored, files));
        }

        let (module, suffix) = self
            .modules
            .iter()
            .filter(|module| module.replaced_in.is_none() || module.replaced_in.as_deref() == owner)
            .find_map(|module| {
                let suffix = match spec.strip_prefix(module.prefix.as_str())? {
                    "" => "",
                    rest => rest.strip_prefix('/')?,
                };
                Some((module, suffix))
            })?;
        let target = match (module.dir.as_str(), suffix) {
            ("", "") => ".".to_owned(),
            ("", suffix) => suffix.to_owned(),
            (dir, "") => dir.to_owned(),
            (dir, suffix) => format!("{dir}/{suffix}"),
        };

        let files: Vec<String> = self.repository.files_in_dir(&target).collect();
        (!files.is_empty()).then_some((target, files))
    }

    fn owner(&self, path: &str) -> Option<&str> {
        self.modules
            .iter()
            .filter(|module| module.replaced_in.is_none())
            .map(|module| module.dir.as_str())
            .filter(|dir| {
                dir.is_empty()
                    || path
                        .strip_prefix(dir)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|dir| dir.len())
    }
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the go command builds only the files of a package whose names end in `.go` in lower case"
)]
fn includes(files: Vec<String>, path: &str, includes: &mut Vec<IncludeRef>) {
    let package_files = files
        .into_iter()
        .filter(|file| file.ends_with(".go") && !file.ends_with("_test.go") && file != path);
    includes.extend(package_files.map(|target| IncludeRef { target }));
}

impl Resolver for GoResolver<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(path.ends_with("_test.go"))
            .with_generated(path.ends_with(".pb.go"));
        if path_flags != SymbolFlags::empty() {
            for symbol in &mut extraction.symbols {
                symbol.flags = symbol.flags.union(path_flags);
            }
        }

        let owner = self.owner(path);
        for directive in directives {
            let target_dir = self.package(&directive.spec, owner).map(|(dir, files)| {
                includes(files, path, &mut extraction.includes);
                dir
            });

            let spec = directive.spec.as_str();
            extraction.bindings.push(ImportBinding {
                alias: directive.names.into_iter().next(),
                target_dir,
                last_segment: spec.rsplit('/').next().unwrap_or(spec).to_owned(),
            });
        }

        extraction
    }
}

// constraint: a vendored `go.mod` belongs to a third-party package, and a checkout can hold thousands of them
fn is_vendored(path: &str) -> bool {
    path.starts_with("vendor/") || path.contains("/vendor/")
}

fn read_go_mod(path_dir: &str, contents: &str, modules: &mut Vec<Module>) {
    let module_dir = if path_dir == "." { "" } else { path_dir };
    for line in contents.lines().map(str::trim) {
        if let Some(module) = line.strip_prefix("module ") {
            let module = module
                .split_once("//")
                .map_or(module, |(module, _)| module)
                .trim();
            if !module.is_empty() {
                modules.push(Module {
                    prefix: module.to_owned(),
                    dir: module_dir.to_owned(),
                    replaced_in: None,
                });
            }
        } else if let Some((from, to)) = local_replace(line)
            && let Some(dir) = join(module_dir, to)
        {
            modules.push(Module {
                prefix: from.to_owned(),
                dir,
                replaced_in: Some(module_dir.to_owned()),
            });
        }
    }
}

// constraint: only a replace onto a local directory changes where a package is; one onto another module path leaves
// the import external
fn local_replace(line: &str) -> Option<(&str, &str)> {
    let line = line.strip_prefix("replace ").unwrap_or(line);
    let (from, to) = line.split_once("=>")?;
    let to = to.split_whitespace().next()?;
    if !to.starts_with("./") && !to.starts_with("../") {
        return None;
    }

    Some((from.split_whitespace().next()?, to))
}
