use std::any::Any;
use std::cmp::Reverse;

use kartta_lang::path::{dir, join};
use kartta_lang::{
    Extraction, ImportBinding, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

use crate::build::{file_constraint, is_skipped};
use crate::walk::CONSTRAINT_FORM;

#[derive(PartialEq)]
pub(crate) struct GoResolver {
    modules: Vec<Module>,
    vendoring: Vec<String>,
}

#[derive(PartialEq)]
struct Module {
    prefix: String,
    dir: String,
    replaced_in: Option<String>,
}

impl GoResolver {
    pub(crate) fn new(repository: &dyn Repository) -> Self {
        let mut modules = Vec::new();
        // constraint: a vendored `go.mod` belongs to a third-party package, and a checkout can hold thousands of them
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
        let mut vendoring: Vec<String> = modules
            .iter()
            .filter(|module| module.replaced_in.is_none())
            .map(|module| module.dir.clone())
            .filter(|dir| repository.contains(&join_dir(dir, "vendor/modules.txt")))
            .collect();
        vendoring.sort_unstable();
        vendoring.dedup();

        Self { modules, vendoring }
    }

    // constraint: a module with no vendor directory of its own is compiled by the module that replaces it onto its
    // directory, with that module's vendor directory and replace directives, as the go command builds the
    // dependencies of its main module; one that vendors keeps its own build
    fn main_module<'a>(&'a self, owner: Option<&'a str>) -> Option<&'a str> {
        let own = owner?;
        if self.vendors(own) {
            return owner;
        }
        self.modules
            .iter()
            .filter(|module| module.dir == own)
            .filter_map(|module| module.replaced_in.as_deref())
            .max_by_key(|replacing| (self.vendors(replacing), replacing.len()))
            .or(owner)
    }

    fn vendors(&self, module_dir: &str) -> bool {
        self.vendoring
            .binary_search_by(|dir| dir.as_str().cmp(module_dir))
            .is_ok()
    }

    // constraint: a vendored build prefers the vendored copy; otherwise the longest module prefix decides alone, and a
    // directory under it that holds no files is not in the repository
    // constraint: the go command applies only the main module's replace directives and vendor directory, so an
    // importing file sees those of the module it belongs to
    // shortcut: a replace that names a version applies to every version of the module; telling them apart needs the
    // versions the module requires
    fn package(
        &self,
        spec: &str,
        owner: Option<&str>,
        repository: &dyn Repository,
    ) -> Option<(String, Vec<String>)> {
        let vendored = match owner {
            None | Some("") => format!("vendor/{spec}"),
            Some(module_dir) => format!("{module_dir}/vendor/{spec}"),
        };
        let files: Vec<String> = repository.files_in_dir(&vendored).collect();
        if !files.is_empty() {
            return Some((vendored, files));
        }

        // constraint: two modules can declare one path, a checkout's nested copy among them; the go command builds
        // the importing file's own module, so its packages win the tie
        let (module, suffix) = self
            .modules
            .iter()
            .filter(|module| module.replaced_in.is_none() || module.replaced_in.as_deref() == owner)
            .filter_map(|module| {
                let suffix = match spec.strip_prefix(module.prefix.as_str())? {
                    rest if module.prefix.is_empty() => rest,
                    "" => "",
                    rest => rest.strip_prefix('/')?,
                };
                Some((module, suffix))
            })
            .min_by_key(|(module, _)| {
                (
                    Reverse(module.prefix.len()),
                    Some(module.dir.as_str()) != owner,
                )
            })?;
        let target = match (module.dir.as_str(), suffix) {
            ("", "") => ".".to_owned(),
            ("", suffix) => suffix.to_owned(),
            (dir, "") => dir.to_owned(),
            (dir, suffix) => format!("{dir}/{suffix}"),
        };

        let files: Vec<String> = repository.files_in_dir(&target).collect();
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
    let package_files = files.into_iter().filter(|file| {
        file.ends_with(".go") && !file.ends_with("_test.go") && !is_skipped(file) && file != path
    });
    includes.extend(package_files.map(IncludeRef::new));
}

impl Resolver for GoResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(path.ends_with("_test.go"))
            .with_generated(path.ends_with(".pb.go"))
            .with_vendored(is_vendored(path));
        extraction.flag_symbols(path_flags);

        let owner = self.main_module(self.owner(path));
        let mut header = None;
        for directive in directives {
            if directive.form == CONSTRAINT_FORM {
                header = Some(directive.spec);
                continue;
            }
            let target_dir =
                self.package(&directive.spec, owner, repository)
                    .map(|(dir, files)| {
                        includes(files, path, &mut extraction.includes);
                        dir
                    });

            let last_segment = package_name(&directive.spec).to_owned();
            let alias = directive.names.into_iter().next();
            extraction
                .bindings
                .push(ImportBinding::new(alias, target_dir, last_segment));
        }
        extraction.build_constraint = file_constraint(path, header);

        extraction
    }

    fn resolves_as(&self, kept: &dyn Resolver) -> bool {
        (kept as &dyn Any).downcast_ref::<Self>() == Some(self)
    }
}

// constraint: a module's major version suffix, `/v2` or gopkg.in's `.v3`, is no part of its package's name, which the
// go command's convention takes from the element before it
fn package_name(spec: &str) -> &str {
    let (_, element) = package_segments(spec);
    match element.rsplit_once('.') {
        Some((head, tail)) if !head.is_empty() && is_major_version(tail) => head,
        _ => element,
    }
}

// constraint: the first is the last segment of `spec`, the second the element its package is named from
pub(crate) fn package_segments(spec: &str) -> (&str, &str) {
    let mut segments = spec.rsplit('/');
    let last = segments.next().unwrap_or(spec);
    let element = match segments.next() {
        Some(previous) if is_major_version(last) => previous,
        _ => last,
    };
    (last, element)
}

pub(crate) fn is_major_version(segment: &str) -> bool {
    segment.len() > 1
        && segment.starts_with('v')
        && segment[1..].bytes().all(|byte| byte.is_ascii_digit())
}

fn join_dir(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_owned()
    } else {
        format!("{dir}/{rest}")
    }
}

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
                    prefix: import_prefix(module).to_owned(),
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

// constraint: the go command names the packages of the standard library's module `std` by their paths below its
// directory, with no prefix (`cmd/go/internal/modload/init.go`, `pathPrefix`); it does so only for a `std` inside
// GOROOT/src, which a repository root cannot tell, so every `std` takes it
fn import_prefix(module: &str) -> &str {
    if module == "std" { "" } else { module }
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
