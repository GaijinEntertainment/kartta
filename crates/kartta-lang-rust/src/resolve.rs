use std::collections::HashSet;
use std::iter;

use kartta_lang::path::{base, dir, join};
use kartta_lang::{
    Extraction, IncludeRef, RawDirective, RawExtraction, Repository, Resolver, SymbolFlags,
};

use crate::walk::MOD_TEST_FORM;

pub(crate) struct Crates<'r> {
    repository: &'r dyn Repository,
    packages: HashSet<String>,
}

impl<'r> Crates<'r> {
    // constraint: a package is a directory that holds a `Cargo.toml`; its manifest is not read, so a target that the
    // manifest places off the conventional layout is not a crate root here
    pub(crate) fn new(repository: &'r dyn Repository) -> Self {
        let packages = repository
            .files_named("Cargo.toml")
            .map(|mut manifest| {
                manifest.truncate(manifest.rfind('/').unwrap_or(0));
                manifest
            })
            .collect();

        Self {
            repository,
            packages,
        }
    }

    fn role(&self, path: &str) -> Role {
        let mut role = Role {
            test: is_integration_test(path),
            entry: false,
            module_root: matches!(base(path), "lib.rs" | "main.rs" | "mod.rs"),
        };
        let mut nearest = true;
        for package in ancestors(path).filter(|package| self.packages.contains(*package)) {
            let relative = if package.is_empty() {
                path
            } else {
                &path[package.len() + 1..]
            };
            if nearest {
                let crate_root = is_crate_root(relative);
                role.module_root |= crate_root;
                role.entry = crate_root && relative != "src/lib.rs";
                nearest = false;
            }
            role.test |= is_integration_test(relative);
        }
        role
    }

    // constraint: the spec is the module's path from the declaring file's module directory, one directory per inline
    // module around the declaration; a `#[path]` override is relative to the declaring file's directory, or inside an
    // inline module to that module's directory
    fn module_file(
        &self,
        directive: &RawDirective,
        path: &str,
        module_root: bool,
    ) -> Option<String> {
        if let Some(spec) = directive.names.first() {
            let mut from = String::new();
            match directive.spec.rsplit_once('/') {
                Some((inline, _)) => {
                    push_module_dir(&mut from, path, module_root);
                    from.push_str(inline);
                }
                None => from.push_str(dir(path)),
            }
            return join(&from, spec).filter(|file| self.repository.contains(file));
        }

        let mut file = String::with_capacity(path.len() + directive.spec.len() + "/mod.rs".len());
        push_module_dir(&mut file, path, module_root);
        file.push_str(&directive.spec);
        file.push_str(".rs");
        if self.repository.contains(&file) {
            return Some(file);
        }
        file.truncate(file.len() - ".rs".len());
        file.push_str("/mod.rs");
        self.repository.contains(&file).then_some(file)
    }
}

// constraint: a file named `lib.rs`, `main.rs` or `mod.rs`, or a crate root, keeps its submodules beside it, and any
// other file in the directory named for its stem
fn push_module_dir(into: &mut String, path: &str, module_root: bool) {
    let (parent, file) = path.rsplit_once('/').unwrap_or(("", path));
    if !parent.is_empty() {
        into.push_str(parent);
        into.push('/');
    }
    if !module_root {
        into.push_str(file.rsplit_once('.').map_or(file, |(stem, _)| stem));
        into.push('/');
    }
}

impl Resolver for Crates<'_> {
    fn resolve(&self, mut raw: RawExtraction, path: &str) -> Extraction {
        let entry_candidates = std::mem::take(&mut raw.entry_candidates);
        let (mut extraction, directives) = raw.into_extraction(path);

        let role = self.role(path);
        let flags = SymbolFlags::empty()
            .with_test(role.test)
            .with_generated(is_generated_path(path));
        for symbol in &mut extraction.symbols {
            symbol.flags = symbol.flags.union(flags);
        }
        for id in entry_candidates.into_iter().filter(|_| role.entry) {
            if let Some(symbol) = extraction.symbols.get_mut(id.0 as usize) {
                symbol.flags = symbol.flags.with_entry(true);
            }
        }

        for directive in &directives {
            let Some(target) = self.module_file(directive, path, role.module_root) else {
                continue;
            };
            if directive.form == MOD_TEST_FORM || role.test {
                extraction.test_includes.push(target.clone());
            }
            extraction.includes.push(IncludeRef { target });
        }

        extraction
    }
}

// constraint: a file is an entry when it is a crate root of its nearest package other than the library, where a
// top-level `fn main` is what the target runs
struct Role {
    test: bool,
    entry: bool,
    module_root: bool,
}

// constraint: the crate roots of a package by Cargo's conventional layout: the library, the main binary, the build
// script, and each binary, example, integration test and benchmark as a single file or as a directory with a
// `main.rs`
fn is_crate_root(relative: &str) -> bool {
    if matches!(relative, "src/lib.rs" | "src/main.rs" | "build.rs") {
        return true;
    }
    ["src/bin/", "examples/", "tests/", "benches/"]
        .iter()
        .find_map(|targets| relative.strip_prefix(targets))
        .is_some_and(|target| {
            target
                .split_once('/')
                .is_none_or(|(_, file)| file == "main.rs")
        })
}

// constraint: below the repository root or a package around the file, a `tests` directory outside `src` holds
// integration tests and their support; this covers a crate whose manifest the tree lacks, and a fixture crate kept
// under `tests/`, while a `tests` module inside `src` stays production code
fn is_integration_test(relative: &str) -> bool {
    relative
        .split('/')
        .take_while(|component| *component != "src")
        .any(|component| component == "tests")
}

fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    iter::successors(Some(path), |at| {
        (!at.is_empty()).then(|| at.rsplit_once('/').map_or("", |(parent, _)| parent))
    })
    .skip(1)
}

fn is_generated_path(path: &str) -> bool {
    let name = base(path);
    name.ends_with(".gen.rs") || name.ends_with("_generated.rs")
}
