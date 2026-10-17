use std::borrow::Cow;
use std::collections::HashMap;
use std::iter;

use kartta_lang::path::{base, dir, join};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver, SymbolFlags,
};

use crate::sqconfig::{SQCONFIG, SqConfig};

pub(crate) struct QuirrelResolver<'r> {
    repository: &'r dyn Repository,
    configs: HashMap<String, SqConfig>,
}

impl<'r> QuirrelResolver<'r> {
    // constraint: a `.sqconfig` that cannot be read still governs the scripts below it, so its mounts and base paths
    // are unknown and only the file beside a script resolves
    pub(crate) fn new(repository: &'r dyn Repository) -> Self {
        let mut configs = HashMap::new();
        for path in repository.files_named(SQCONFIG) {
            let config_dir = dir(&path);
            let config = repository.read(&path).map_or_else(
                |_| SqConfig::default(),
                |bytes| SqConfig::parse(config_dir, &String::from_utf8_lossy(&bytes)),
            );
            configs.insert(config_dir.to_owned(), config);
        }

        Self {
            repository,
            configs,
        }
    }

    // constraint: the script tooling reads the `.sqconfig` nearest the script, in its directory or above it, and that
    // file alone
    fn nearest(&self, path: &str) -> Option<&SqConfig> {
        if self.configs.is_empty() {
            return None;
        }

        let mut config_dir = dir(path);
        loop {
            if let Some(config) = self.configs.get(config_dir) {
                return Some(config);
            }
            if config_dir == "." {
                return None;
            }
            config_dir = dir(config_dir);
        }
    }

    // constraint: the loader resolves `%name/rest` below the directory the config mounts as `name`, and any other
    // `.nut` specifier beside the script, else below the first base path that holds it; it appends no extension, so a
    // specifier without `.nut` is a native module
    fn configured(&self, config: &SqConfig, spec: &str, path: &str) -> Option<String> {
        if let Some(mounted) = spec.strip_prefix('%') {
            let (name, rest) = mounted.split_once('/')?;
            let mount_dir = config.mounts.get(name)?;
            return self.loaded(join(mount_dir, &forward_slashes(rest))?);
        }
        if !names_a_script(spec) {
            return None;
        }

        let spec = forward_slashes(spec);
        iter::once(dir(path))
            .chain(config.base_paths.iter().map(String::as_str))
            .find_map(|base_dir| self.loaded(join(base_dir, &spec)?))
    }

    // constraint: the loader reads shipped scripts from a vromfs, whose lookup lowercases the path over names the
    // packer stores lowercased, so a path that names no file loads the one file that differs from it only in letter
    // case
    fn loaded(&self, candidate: String) -> Option<String> {
        if self.repository.contains(&candidate) {
            return Some(candidate);
        }
        let mut matches = self.repository.files_ignoring_case(&candidate);
        let only = matches.next()?;

        matches.next().is_none().then_some(only)
    }

    // constraint: without a `.sqconfig` the mounts are unknown, so a `%root/rest` path takes the nearest ancestor of
    // the script that holds `root/rest`, as each game mounts its own roots
    fn guessed(&self, spec: &str, path: &str) -> Option<String> {
        let spec = forward_slashes(spec);
        if let Some(rest) = spec.strip_prefix('%') {
            return self.nearest_ancestor(rest, path);
        }
        if spec.starts_with("./") || spec.starts_with("../") {
            return self.probe(&join(dir(path), &spec)?);
        }
        if spec.contains('/') || names_a_script(&spec) {
            return self.nearest_ancestor(&spec, path);
        }

        None
    }

    fn nearest_ancestor(&self, relative: &str, path: &str) -> Option<String> {
        let mut ancestor = dir(path);
        loop {
            let candidate = if ancestor == "." {
                relative.to_owned()
            } else {
                format!("{ancestor}/{relative}")
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

    fn probe(&self, candidate: &str) -> Option<String> {
        if self.repository.contains(candidate) {
            return Some(candidate.to_owned());
        }
        if names_a_script(candidate) {
            return None;
        }

        let with_extension = format!("{candidate}.nut");
        self.repository
            .contains(&with_extension)
            .then_some(with_extension)
    }
}

impl Resolver for QuirrelResolver<'_> {
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

        let config = self.nearest(path);
        for directive in directives {
            let target = match config {
                Some(config) => self.configured(config, &directive.spec, path),
                None => self.guessed(&directive.spec, path),
            };
            match target {
                Some(target) => extraction.includes.push(IncludeRef { target }),
                None => extraction.imports.push(ImportRef {
                    module: directive.spec,
                    names: directive.names,
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

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the loader tells a script from a native module by the lower-case `.nut` the specifier ends with"
)]
fn names_a_script(spec: &str) -> bool {
    spec.ends_with(".nut")
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
